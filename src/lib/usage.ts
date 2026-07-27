import { getSessionUpdate, getSessionUpdateKind } from "./acpTranscript";
import type { UsageSnapshot, UsageWindow } from "./types";

/** Per-session usage state accumulated from ACP (and opportunistic text). */
export type SessionUsageState = {
  contextUsed: number | null;
  contextSize: number | null;
  costAmount: number | null;
  costCurrency: string | null;
  /** Rate-limit / quota windows keyed by stable id. */
  windows: Record<string, UsageWindow>;
  /** Provider balance rows (OpenCode model → DeepSeek/OpenRouter/Go…). */
  providerWindows: Record<string, UsageWindow>;
  providerLabel: string | null;
  providerModel: string | null;
  refreshedAt: string | null;
  source: string | null;
  /** Last turn's token split, from the session/prompt response. */
  turnTokens: TurnTokens | null;
};

/**
 * End-of-turn token split. Reported in the `session/prompt` *response*, which
 * every agent fills in — including the ones that never send `usage_update`.
 */
export type TurnTokens = {
  input: number | null;
  output: number | null;
  cached: number | null;
  reasoning: number | null;
  total: number | null;
};

export function emptySessionUsage(): SessionUsageState {
  return {
    contextUsed: null,
    contextSize: null,
    costAmount: null,
    costCurrency: null,
    windows: {},
    providerWindows: {},
    providerLabel: null,
    providerModel: null,
    refreshedAt: null,
    source: null,
    turnTokens: null,
  };
}

/** Snapshot returned by Rust `probe_provider_usage`. */
export type ProviderUsageProbe = {
  provider: string;
  providerLabel: string;
  model: string | null;
  modelLabel: string | null;
  windows: Array<{
    id: string;
    label: string;
    percentage: number | null;
    detail: string | null;
    kind: string;
  }>;
  note: string | null;
  refreshedAt: string;
  source: string;
  ok: boolean;
};

export function mergeProviderProbe(
  prev: SessionUsageState | undefined,
  probe: ProviderUsageProbe,
  now = new Date()
): SessionUsageState {
  const base = prev ?? emptySessionUsage();
  const providerWindows: Record<string, UsageWindow> = {};
  for (const w of probe.windows) {
    providerWindows[w.id] = {
      id: w.id,
      label: w.label,
      percentage: w.percentage,
      detail: w.detail,
      kind: w.kind === "rate_limit" ? "rate_limit" : "provider",
    };
  }
  const sourceBits = [base.source, probe.source].filter(Boolean);
  return {
    ...base,
    providerWindows,
    providerLabel: probe.providerLabel,
    providerModel: probe.modelLabel ?? probe.model,
    refreshedAt: now.toISOString(),
    source: sourceBits.length > 0 ? sourceBits.join(" + ") : probe.source,
  };
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function asNumber(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim() !== "") {
    const n = Number(value);
    return Number.isFinite(n) ? n : null;
  }
  return null;
}

/** utilization may be 0–1 fraction or 0–100 percent. */
export function normalizeUtilizationPercent(value: number | null | undefined): number | null {
  if (value == null || !Number.isFinite(value)) return null;
  if (value < 0) return 0;
  if (value <= 1) return Math.round(value * 1000) / 10; // keep one decimal for fractions
  return Math.min(100, Math.round(value * 10) / 10);
}

function formatTokenCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1000)}K`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return String(Math.round(n));
}

/** `13K in · 29 out · 2.8K cached` — omits whatever the agent left null. */
function formatTurnTokens(tokens: TurnTokens | null): string | null {
  if (!tokens) return null;
  const parts: string[] = [];
  if (tokens.input != null) parts.push(`${formatTokenCount(tokens.input)} in`);
  if (tokens.output != null) parts.push(`${formatTokenCount(tokens.output)} out`);
  if (tokens.cached != null && tokens.cached > 0) {
    parts.push(`${formatTokenCount(tokens.cached)} cached`);
  }
  if (tokens.reasoning != null && tokens.reasoning > 0) {
    parts.push(`${formatTokenCount(tokens.reasoning)} thinking`);
  }
  if (parts.length === 0 && tokens.total != null) {
    parts.push(`${formatTokenCount(tokens.total)} total`);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

function formatCost(amount: number, currency: string): string {
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: currency || "USD",
      minimumFractionDigits: 2,
      maximumFractionDigits: 4,
    }).format(amount);
  } catch {
    return `${amount.toFixed(4)} ${currency || "USD"}`;
  }
}

function formatResetHint(resetsAt: unknown): string | null {
  if (resetsAt == null) return null;
  let date: Date | null = null;
  if (typeof resetsAt === "number") {
    // seconds vs ms
    date = new Date(resetsAt < 1e12 ? resetsAt * 1000 : resetsAt);
  } else if (typeof resetsAt === "string") {
    const parsed = Date.parse(resetsAt);
    if (!Number.isNaN(parsed)) date = new Date(parsed);
  }
  if (!date || Number.isNaN(date.getTime())) return null;
  const ms = date.getTime() - Date.now();
  if (ms <= 0) return "resets soon";
  const mins = Math.round(ms / 60_000);
  if (mins < 60) return `resets in ${mins}m`;
  const hours = Math.round(mins / 60);
  if (hours < 48) return `resets in ${hours}h`;
  const days = Math.round(hours / 24);
  return `resets in ${days}d`;
}

function claudeRateLimitLabel(type: string | undefined): { id: string; label: string } {
  switch (type) {
    case "five_hour":
      return { id: "five-hour", label: "5-hour limit" };
    case "seven_day":
      return { id: "weekly", label: "Weekly limit" };
    case "seven_day_opus":
      return { id: "weekly-opus", label: "Weekly (Opus)" };
    case "seven_day_sonnet":
      return { id: "weekly-sonnet", label: "Weekly (Sonnet)" };
    case "seven_day_overage_included":
      return { id: "weekly-overage", label: "Weekly overage" };
    case "overage":
      return { id: "overage", label: "Overage" };
    default:
      return { id: type ? `rate-${type}` : "rate-limit", label: type ? `Limit (${type})` : "Rate limit" };
  }
}

/** Parse Claude `_meta["_claude/rateLimit"]` (SDKRateLimitInfo). */
export function parseClaudeRateLimitMeta(meta: unknown): UsageWindow[] {
  const root = asRecord(meta);
  if (!root) return [];
  const info = asRecord(root["_claude/rateLimit"] ?? root["claude/rateLimit"] ?? root.rateLimit);
  if (!info) return [];

  const type = typeof info.rateLimitType === "string" ? info.rateLimitType : undefined;
  const { id, label } = claudeRateLimitLabel(type);
  const utilization = normalizeUtilizationPercent(asNumber(info.utilization));
  const status = typeof info.status === "string" ? info.status : null;
  const reset = formatResetHint(info.resetsAt ?? info.resets_at);
  const parts: string[] = [];
  if (status && status !== "allowed") parts.push(status.replace(/_/g, " "));
  if (reset) parts.push(reset);
  if (info.isUsingOverage === true || info.overageInUse === true) parts.push("using overage");

  return [
    {
      id,
      label,
      percentage: utilization,
      detail: parts.length > 0 ? parts.join(" · ") : null,
      kind: "rate_limit",
    },
  ];
}

/**
 * Claude experimental getUsage shape sometimes appears nested in _meta
 * (`rate_limits.five_hour.utilization`, etc.).
 */
export function parseClaudeRateLimitsBucket(meta: unknown): UsageWindow[] {
  const root = asRecord(meta);
  if (!root) return [];
  const claude = asRecord(root["_claude/rateLimit"] ?? root["_claude/usage"] ?? root.claude);
  const bucket =
    asRecord(root.rate_limits) ??
    asRecord(claude?.rate_limits) ??
    asRecord(asRecord(root["_claude/rateLimit"])?.rate_limits);
  if (!bucket) return [];

  const out: UsageWindow[] = [];
  const map: Array<[string, string, string]> = [
    ["five_hour", "five-hour", "5-hour limit"],
    ["seven_day", "weekly", "Weekly limit"],
    ["seven_day_opus", "weekly-opus", "Weekly (Opus)"],
    ["seven_day_sonnet", "weekly-sonnet", "Weekly (Sonnet)"],
  ];
  for (const [key, id, label] of map) {
    const win = asRecord(bucket[key]);
    if (!win) continue;
    const utilization = normalizeUtilizationPercent(asNumber(win.utilization));
    if (utilization == null && win.resets_at == null && win.resetsAt == null) continue;
    const reset = formatResetHint(win.resets_at ?? win.resetsAt);
    out.push({
      id,
      label,
      percentage: utilization,
      detail: reset,
      kind: "rate_limit",
    });
  }
  return out;
}

/**
 * Parse Claude Code `/usage` output.
 *
 * This is the *only* way to get plan limits on demand: `_claude/rateLimit` meta
 * rides on a `rate_limit_event`, which the CLI emits only when a limit is
 * actually being approached — so a healthy account reports nothing all day.
 * `/usage` is a local slash command (no model turn, no tokens), and the adapter
 * forwards its stdout as ordinary assistant text.
 *
 * Live shape (claude-agent-acp 0.61.0):
 *   Current session: 18% used · resets Jul 28, 3:10am (Asia/Tokyo)
 *   Current week (all models): 68% used · resets Jul 30, 7pm (Asia/Tokyo)
 *   Current week (Opus): 4% used · resets Jul 30, 7pm (Asia/Tokyo)
 *
 * Note these are percentages *used*, unlike Codex's "% left".
 */
export function parseClaudeUsageText(text: string): UsageWindow[] {
  if (!text || !text.includes("%")) return [];
  const out: UsageWindow[] = [];
  const re =
    /^\s*Current\s+(session|week)\s*(?:\(([^)]*)\))?\s*:\s*(\d+(?:\.\d+)?)\s*%\s*used([^\n]*)/gim;

  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    const scope = match[1].toLowerCase();
    const qualifier = (match[2] ?? "").trim().toLowerCase();
    const used = Number(match[3]);
    if (!Number.isFinite(used)) continue;

    let id: string;
    let label: string;
    if (scope === "session") {
      // Claude's rolling session window is the 5-hour limit.
      id = "five-hour";
      label = "5-hour limit";
    } else if (qualifier && qualifier !== "all models") {
      // Per-model weekly buckets (Opus/Sonnet) get their own row.
      const pretty = qualifier.replace(/\b\w/g, (c) => c.toUpperCase());
      id = `weekly-${qualifier.replace(/\s+/g, "-")}`;
      label = `Weekly (${pretty})`;
    } else {
      id = "weekly";
      label = "Weekly limit";
    }

    // `· resets Jul 30, 7pm (Asia/Tokyo)` → `resets Jul 30, 7pm (Asia/Tokyo)`
    const detail = (match[4] ?? "").replace(/^[\s·\-]+/, "").trim();
    out.push({
      id,
      label,
      percentage: Math.max(0, Math.min(100, Math.round(used * 10) / 10)),
      detail: detail || null,
      kind: "rate_limit",
    });
  }
  return out;
}

/**
 * Parse Codex `/status` markdown lines, e.g.
 * `**5h limit:** 42% left · resets at …`
 */
export function parseCodexStatusRateLimits(text: string): UsageWindow[] {
  if (!text || !text.includes("%")) return [];
  const out: UsageWindow[] = [];
  // Codex /status lines look like: `**5h limit:** 58% left · resets in 2h`
  // Optional vendor prefix: `**ChatGPT 5h limit:** …`
  const re =
    /\*\*([^*]*?((?:\d+[mhd])\s+limit|Weekly limit)):\*\*\s*(\d+(?:\.\d+)?)%\s*left([^\n]*)/gi;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    const fullLabel = match[1].trim();
    const durationLabel = match[2].trim();
    const percentLeft = Number(match[3]);
    if (!Number.isFinite(percentLeft)) continue;
    const used = Math.max(0, Math.min(100, Math.round((100 - percentLeft) * 10) / 10));
    const tail = (match[4] ?? "").replace(/^[·\-\s]+/, "").trim();
    const id = codexWindowId(durationLabel);
    out.push({
      id,
      label: humanizeCodexLabel(fullLabel || durationLabel),
      percentage: used,
      detail: tail || `${percentLeft}% left`,
      kind: "rate_limit",
    });
  }
  return out;
}

function codexWindowId(label: string): string {
  const lower = label.toLowerCase();
  if (lower.includes("weekly")) return "weekly";
  const h = lower.match(/(\d+)\s*h/);
  if (h) {
    if (h[1] === "5") return "five-hour";
    return `${h[1]}h`;
  }
  const m = lower.match(/(\d+)\s*m/);
  if (m) return `${m[1]}m`;
  const d = lower.match(/(\d+)\s*d/);
  if (d) return d[1] === "7" ? "weekly" : `${d[1]}d`;
  return `limit-${lower.replace(/\s+/g, "-")}`;
}

function humanizeCodexLabel(label: string): string {
  const trimmed = label.trim();
  const lower = trimmed.toLowerCase();
  if (lower === "weekly limit" || lower.endsWith(" weekly limit")) {
    return trimmed.replace(/weekly limit/i, "Weekly limit");
  }
  const h = lower.match(/(\d+)\s*h\s*limit/);
  if (h) {
    return trimmed.replace(new RegExp(`${h[1]}\\s*h\\s*limit`, "i"), `${h[1]}-hour limit`);
  }
  const m = lower.match(/(\d+)\s*m\s*limit/);
  if (m) {
    return trimmed.replace(new RegExp(`${m[1]}\\s*m\\s*limit`, "i"), `${m[1]}-minute limit`);
  }
  return trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
}

/** Extract usage fields from an ACP `session/update` payload. */
export function extractUsageFromAcpData(data: unknown): {
  contextUsed: number | null;
  contextSize: number | null;
  costAmount: number | null;
  costCurrency: string | null;
  rateWindows: UsageWindow[];
} | null {
  const update = getSessionUpdate(data);
  if (!update) return null;
  const kind = getSessionUpdateKind(update);
  if (kind !== "usage_update" && kind !== "context_update") return null;

  const contextUsed = asNumber(update.used);
  const contextSize = asNumber(update.size);
  const costObj = asRecord(update.cost);
  const costAmount = costObj ? asNumber(costObj.amount) : null;
  const costCurrency =
    costObj && typeof costObj.currency === "string" ? costObj.currency : costObj ? "USD" : null;

  const meta = update._meta ?? update.meta;
  const rateWindows = [
    ...parseClaudeRateLimitMeta(meta),
    ...parseClaudeRateLimitsBucket(meta),
    // Future / vendor-specific nests
    ...parseCodexMetaRateLimits(meta),
  ];

  // Dedupe by id (later wins)
  const byId = new Map<string, UsageWindow>();
  for (const w of rateWindows) byId.set(w.id, w);

  return {
    contextUsed,
    contextSize,
    costAmount,
    costCurrency,
    rateWindows: [...byId.values()],
  };
}

/** Best-effort: Codex (or others) may put rate limit snapshots under `_meta`. */
function parseCodexMetaRateLimits(meta: unknown): UsageWindow[] {
  const root = asRecord(meta);
  if (!root) return [];
  const candidates = [
    root["_codex/rateLimits"],
    root["_codex/rate_limits"],
    root.rateLimits,
    asRecord(root.codex)?.rateLimits,
  ];
  const out: UsageWindow[] = [];
  for (const c of candidates) {
    if (!c) continue;
    // Map of limitId → snapshot, or a single snapshot with primary/secondary
    if (Array.isArray(c)) {
      for (const item of c) out.push(...windowsFromCodexSnapshot(item));
    } else if (asRecord(c)?.primary || asRecord(c)?.secondary) {
      out.push(...windowsFromCodexSnapshot(c));
    } else {
      const rec = asRecord(c);
      if (!rec) continue;
      for (const value of Object.values(rec)) {
        const entry = asRecord(value);
        const snap = entry?.snapshot ?? entry;
        out.push(...windowsFromCodexSnapshot(snap, typeof entry?.limitName === "string" ? entry.limitName : undefined));
      }
    }
  }
  return out;
}

function windowsFromCodexSnapshot(snapshot: unknown, namePrefix?: string): UsageWindow[] {
  const snap = asRecord(snapshot);
  if (!snap) return [];
  const out: UsageWindow[] = [];
  for (const key of ["primary", "secondary"] as const) {
    const win = asRecord(snap[key]);
    if (!win) continue;
    const usedPercent = asNumber(win.usedPercent ?? win.used_percent);
    if (usedPercent == null) continue;
    const mins = asNumber(win.windowDurationMins ?? win.window_duration_mins);
    const label = codexDurationLabel(mins, namePrefix);
    const id = codexWindowId(label);
    const reset = formatResetHint(win.resetsAt ?? win.resets_at);
    const left = Math.max(0, Math.round((100 - usedPercent) * 10) / 10);
    out.push({
      id,
      label,
      percentage: Math.max(0, Math.min(100, Math.round(usedPercent * 10) / 10)),
      detail: reset ? `${left}% left · ${reset}` : `${left}% left`,
      kind: "rate_limit",
    });
  }
  return out;
}

function codexDurationLabel(mins: number | null, prefix?: string): string {
  let base = "Limit";
  if (mins != null) {
    if (mins < 60) base = `${Math.round(mins)}m limit`;
    else if (mins < 1440) base = `${Math.round(mins / 60)}h limit`;
    else if (mins < 10080) base = `${Math.round(mins / 1440)}d limit`;
    else base = "Weekly limit";
  }
  const human = humanizeCodexLabel(base);
  return prefix ? `${prefix} ${human}` : human;
}

export function mergeUsageFromAcp(
  prev: SessionUsageState | undefined,
  data: unknown,
  now = new Date()
): SessionUsageState | null {
  const extracted = extractUsageFromAcpData(data);
  if (!extracted) return null;
  const base = prev ?? emptySessionUsage();
  const windows = { ...base.windows };
  for (const w of extracted.rateWindows) {
    windows[w.id] = w;
  }
  return {
    ...base,
    contextUsed: extracted.contextUsed ?? base.contextUsed,
    contextSize: extracted.contextSize ?? base.contextSize,
    costAmount: extracted.costAmount ?? base.costAmount,
    costCurrency: extracted.costCurrency ?? base.costCurrency,
    windows,
    refreshedAt: now.toISOString(),
    source: base.source?.includes("usage_update")
      ? base.source
      : base.source
        ? `${base.source} + acp usage_update`
        : "acp usage_update",
  };
}

/**
 * Pull the end-of-turn token split out of a `session/prompt` response.
 *
 * This is the only usage Grok ever reports — it sends no `usage_update` at all
 * (verified against `grok agent stdio` 0.2.104). The other three agents fill in
 * the same field, so this also adds the input/output/cached split they omit
 * from `usage_update`.
 *
 * Shapes seen in the wild:
 * - ACP `PromptResponse.usage`: `{ inputTokens, outputTokens, totalTokens, … }`
 * - Grok:  `_meta.{ totalTokens, inputTokens, outputTokens, cachedReadTokens }`
 * - Codex: `_meta.quota.token_count.{ …, cachedInputTokens, reasoningOutputTokens }`
 */
export function extractTurnTokens(data: unknown): TurnTokens | null {
  const root = asRecord(data);
  if (!root) return null;

  // Unwrap common envelopes:
  // - JSON-RPC response: { result: { _meta / usage } }
  // - ACP notification:  { method, params: { update: { usage } } }  (Grok)
  // - Bare result / update object
  let result = asRecord(root.result) ?? root;
  if (typeof root.method === "string") {
    const params = asRecord(root.params) ?? root;
    const update = asRecord(params.update) ?? params;
    // Only turn_completed (and prompt results) carry usage. Ignore model_changed etc.
    const kind =
      (typeof update.sessionUpdate === "string" && update.sessionUpdate) ||
      (typeof params.sessionUpdate === "string" && params.sessionUpdate) ||
      "";
    if (kind && kind !== "turn_completed" && !asRecord(update.usage) && !asRecord(params.usage)) {
      return null;
    }
    result = update;
  }

  const meta = asRecord(result._meta);
  const candidates = [
    asRecord(result.usage),
    asRecord(meta?.usage),
    meta,
    result,
    asRecord(asRecord(meta?.quota)?.token_count),
    asRecord(asRecord(meta?.quota)?.tokenCount),
  ].filter(Boolean) as Record<string, unknown>[];
  if (candidates.length === 0) return null;

  const pick = (...keys: string[]): number | null => {
    for (const source of candidates) {
      for (const key of keys) {
        const n = asNumber(source[key]);
        if (n != null) return n;
      }
    }
    return null;
  };

  const input = pick("inputTokens", "input_tokens");
  const output = pick("outputTokens", "output_tokens");
  const cached = pick(
    "cachedReadTokens",
    "cached_read_tokens",
    "cachedInputTokens",
    "cached_input_tokens"
  );
  const reasoning = pick(
    "thoughtTokens",
    "thought_tokens",
    "reasoningOutputTokens",
    "reasoning_output_tokens",
    "reasoningTokens"
  );
  const total = pick("totalTokens", "total_tokens");

  if (input == null && output == null && total == null) return null;
  return { input, output, cached, reasoning, total };
}

/**
 * Merge a turn result into usage state.
 *
 * `contextUsed` is only *seeded* here, never overwritten: `usage_update` is the
 * authoritative context figure for agents that send it, and a turn total is not
 * the same number as tokens-in-context once history is compacted.
 */
export function mergeUsageFromPromptResult(
  prev: SessionUsageState | undefined,
  data: unknown,
  now = new Date()
): SessionUsageState | null {
  const tokens = extractTurnTokens(data);
  if (!tokens) return null;
  const base = prev ?? emptySessionUsage();
  const label = "acp prompt result";
  return {
    ...base,
    turnTokens: tokens,
    contextUsed: base.contextUsed ?? tokens.total ?? tokens.input,
    refreshedAt: now.toISOString(),
    source: base.source?.includes(label)
      ? base.source
      : base.source
        ? `${base.source} + ${label}`
        : label,
  };
}

/**
 * Seed the context ceiling from `session/ready`.
 *
 * Agents that never send `usage_update` still advertise a per-model ceiling at
 * session setup, and without it `used` has nothing to be a percentage of.
 */
export function seedContextSize(
  prev: SessionUsageState | undefined,
  size: number | null | undefined
): SessionUsageState | null {
  if (size == null || !Number.isFinite(size) || size <= 0) return null;
  const base = prev ?? emptySessionUsage();
  // A live usage_update always wins — this is only a floor for agents with none.
  if (base.contextSize != null) return null;
  return { ...base, contextSize: size };
}

export function mergeUsageFromText(
  prev: SessionUsageState | undefined,
  text: string,
  now = new Date()
): SessionUsageState | null {
  // Codex /status, Claude /usage and Grok /usage-style text all land here.
  const rateWindows = [
    ...parseCodexStatusRateLimits(text),
    ...parseClaudeUsageText(text),
    ...parseGrokCostText(text),
  ];
  if (rateWindows.length === 0) return null;
  const base = prev ?? emptySessionUsage();
  const windows = { ...base.windows };
  for (const w of rateWindows) windows[w.id] = w;
  return {
    ...base,
    windows,
    refreshedAt: now.toISOString(),
    source: base.source?.includes("status") || base.source?.includes("cost")
      ? base.source
      : base.source
        ? `${base.source} + agent text`
        : "agent text",
  };
}

/**
 * Parse Grok TUI `/usage` (or `/cost`) text if it ever shows up in chat, e.g.
 * `Weekly limit: 5%` / `Next reset: August 1, 01:12`.
 */
export function parseGrokCostText(text: string): UsageWindow[] {
  if (!text) return [];
  const out: UsageWindow[] = [];
  const weekly = text.match(/Weekly\s+limit\s*:\s*(\d+(?:\.\d+)?)\s*%/i);
  if (weekly) {
    const pct = Number(weekly[1]);
    if (Number.isFinite(pct)) {
      const resetLine = text.match(/Next\s+reset\s*:\s*([^\n]+)/i);
      out.push({
        id: "weekly",
        label: "Weekly limit",
        percentage: Math.max(0, Math.min(100, pct)),
        detail: resetLine ? `resets ${resetLine[1].trim()}` : null,
        kind: "rate_limit",
      });
    }
  }
  const monthly = text.match(/Monthly\s+limit\s*:\s*(\d+(?:\.\d+)?)\s*%/i);
  if (monthly) {
    const pct = Number(monthly[1]);
    if (Number.isFinite(pct)) {
      out.push({
        id: "monthly",
        label: "Monthly limit",
        percentage: Math.max(0, Math.min(100, pct)),
        detail: null,
        kind: "rate_limit",
      });
    }
  }
  return out;
}

/**
 * Parse Grok ACP `_x.ai/billing` (what the TUI `/usage` panel actually loads).
 *
 * Live shape (grok 0.2.104):
 * ```
 * { config: { creditUsagePercent: 6,
 *             currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY", start, end },
 *             billingPeriodEnd: "…" },
 *   subscription_tier: "SuperGrok" }
 * ```
 */
export function parseGrokBilling(data: unknown): {
  windows: UsageWindow[];
  tier: string | null;
} | null {
  const root = asRecord(data);
  if (!root) return null;
  const config = asRecord(root.config) ?? root;
  const pct = asNumber(config.creditUsagePercent);
  if (pct == null) return null;

  const period = asRecord(config.currentPeriod);
  const periodType =
    (typeof period?.type === "string" && period.type) ||
    (typeof config.periodType === "string" && config.periodType) ||
    "";
  const isWeekly = /weekly/i.test(periodType) || !periodType;
  const isMonthly = /monthly/i.test(periodType);
  const id = isMonthly ? "monthly" : "weekly";
  const label = isMonthly ? "Monthly limit" : "Weekly limit";

  const end =
    period?.end ??
    config.billingPeriodEnd ??
    config.billing_period_end ??
    null;
  const reset = formatResetHint(end);
  // Prefer a wall-clock reset when we have a real date (matches TUI "Next reset").
  let detail = reset;
  if (typeof end === "string" || typeof end === "number") {
    const ms =
      typeof end === "number"
        ? end < 1e12
          ? end * 1000
          : end
        : Date.parse(end);
    if (!Number.isNaN(ms)) {
      try {
        detail = `resets ${new Date(ms).toLocaleString(undefined, {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })}`;
      } catch {
        /* keep relative */
      }
    }
  }

  const tier =
    (typeof root.subscription_tier === "string" && root.subscription_tier) ||
    (typeof root.subscriptionTier === "string" && root.subscriptionTier) ||
    null;

  // creditUsagePercent is already 0–100 (live: 6 ≈ TUI "Weekly limit: 5%").
  // Only treat values in (0, 1] as fractions if the field ever changes shape.
  const percentage =
    pct > 0 && pct <= 1
      ? normalizeUtilizationPercent(pct) ?? pct * 100
      : Math.max(0, Math.min(100, pct));

  return {
    windows: [
      {
        id,
        label,
        percentage,
        detail,
        kind: "rate_limit",
      },
    ],
    tier,
  };
}

/** Merge a live `_x.ai/billing` probe into session usage state. */
export function mergeGrokBilling(
  prev: SessionUsageState | undefined,
  data: unknown,
  now = new Date()
): SessionUsageState | null {
  const parsed = parseGrokBilling(data);
  if (!parsed || parsed.windows.length === 0) return null;
  const base = prev ?? emptySessionUsage();
  const windows = { ...base.windows };
  for (const w of parsed.windows) windows[w.id] = w;
  const label = "grok _x.ai/billing";
  const tierBit = parsed.tier ? ` · ${parsed.tier}` : "";
  return {
    ...base,
    windows,
    refreshedAt: now.toISOString(),
    source: base.source?.includes(label)
      ? base.source
      : base.source
        ? `${base.source} + ${label}${tierBit}`
        : `${label}${tierBit}`,
  };
}

function formatClock(iso: string | null): string {
  if (!iso) return "Not connected";
  try {
    return new Date(iso).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return iso;
  }
}

/** Build the right-panel snapshot for the active agent/session. */
export function buildUsageSnapshot(args: {
  agentId: string;
  agentLabel: string;
  state: SessionUsageState | undefined;
  connected: boolean;
}): UsageSnapshot {
  const { agentId, agentLabel, state, connected } = args;
  const windows: UsageWindow[] = [];
  const isOpenCode = agentId === "opencode";

  // Context window (session-level, standard ACP)
  const used = state?.contextUsed ?? null;
  const size = state?.contextSize ?? null;
  let contextPct: number | null = null;
  let contextDetail: string | null = null;
  if (used != null && size != null && size > 0) {
    contextPct = Math.max(0, Math.min(100, Math.round((used / size) * 1000) / 10));
    contextDetail = `${formatTokenCount(used)} / ${formatTokenCount(size)} tokens`;
  } else if (used != null) {
    contextDetail = `${formatTokenCount(used)} tokens used`;
  }
  windows.push({
    id: "context",
    label: "Context window",
    percentage: contextPct,
    detail: contextDetail,
    kind: "context",
  });

  // Cost (optional)
  if (state?.costAmount != null) {
    windows.push({
      id: "cost",
      label: "Session cost",
      percentage: null,
      detail: formatCost(state.costAmount, state.costCurrency ?? "USD"),
      kind: "cost",
    });
  }

  // Last turn's token split (session/prompt response — every agent fills this in).
  const turnDetail = formatTurnTokens(state?.turnTokens ?? null);
  if (turnDetail) {
    windows.push({
      id: "turn",
      label: "Last turn",
      percentage: null,
      detail: turnDetail,
      // `cost` kind renders detail as the primary value — no meter, no "N/A".
      kind: "cost",
    });
  }

  // Provider balance (OpenCode: based on selected provider/model)
  const providerRows = Object.values(state?.providerWindows ?? {});
  if (providerRows.length > 0) {
    windows.push(...providerRows);
  } else if (isOpenCode) {
    windows.push({
      id: "provider-balance",
      label: "Provider balance",
      percentage: null,
      detail: connected
        ? "Refresh to query the active model’s provider API"
        : "Start OpenCode, then refresh",
      kind: "provider",
    });
  }

  // Rate limits reported by adapter (Claude/Codex) — skip plan placeholders when
  // OpenCode already contributed provider rate-limit rows (e.g. Go ceilings).
  const rateWindows = Object.values(state?.windows ?? {}).filter(
    (w) => w.kind === "rate_limit" || w.kind == null
  );
  const isGrok = agentId === "grok-build" || agentId === "grok";
  const wantsPlanLimits =
    agentId === "codex" ||
    agentId === "claude-code" ||
    agentId === "claude" ||
    isGrok;
  const byId = new Map(rateWindows.map((w) => [w.id, w]));
  // Don't duplicate ids already shown from provider probe (Go 5h/weekly).
  for (const row of providerRows) {
    byId.delete(row.id);
  }

  // Plan limits are shown only when the agent actually reported them. ACP has no
  // rate-limit field at all, so a permanent "5-hour limit — N/A" pair was two
  // dead rows that made a working Usage panel read as broken; the absence is
  // explained once in `note` instead.
  // Grok: weekly comes from `_x.ai/billing` (TUI /usage), not Claude meta.
  if (wantsPlanLimits) {
    const slots = isGrok
      ? (["weekly", "monthly"] as const)
      : (["five-hour", "weekly"] as const);
    for (const slot of slots) {
      if (providerRows.some((r) => r.id === slot)) continue;
      const row = byId.get(slot);
      if (!row) continue;
      windows.push(row);
      byId.delete(slot);
    }
  }

  if (byId.size > 0) {
    const extras = [...byId.values()];
    const order = (id: string) => {
      if (id === "five-hour" || id === "5h") return 0;
      if (id === "weekly") return 1;
      if (id.startsWith("weekly")) return 2;
      return 10;
    };
    extras.sort((a, b) => order(a.id) - order(b.id) || a.label.localeCompare(b.label));
    windows.push(...extras);
  }

  // Honest empty-state labels: never imply a live % when we only know plan ceilings.
  for (const w of windows) {
    if (w.percentage == null && !w.detail) {
      if (w.kind === "context") {
        // Say what to *do*. Every ACP agent reports context only once a turn has
        // produced tokens, so before the first message there is nothing to wait
        // for — the old "Waiting for usage_update" read as a malfunction.
        w.detail = connected ? "Send a message to fill this in" : "Not connected";
      } else if (w.kind === "rate_limit") {
        w.detail = "No live remaining % from agent";
      } else if (w.kind === "provider") {
        w.detail = "Unavailable via public API";
      }
    }
  }

  /**
   * Did the agent ever actually report anything? `refreshedAt` alone is not
   * evidence: a manual refresh stamps it even when the probe returned nothing,
   * which used to replace the "send a message" hint with a bare
   * "Source: manual refresh".
   */
  const hasUsageData =
    state != null &&
    (state.contextUsed != null ||
      state.turnTokens != null ||
      state.costAmount != null ||
      Object.keys(state.windows).length > 0 ||
      Object.keys(state.providerWindows).length > 0);

  let note: string | null = null;
  if (state?.providerLabel) {
    const modelBit = state.providerModel ? ` · ${state.providerModel}` : "";
    note = `${state.providerLabel}${modelBit}`;
    if (state.source) note += ` · ${state.source}`;
    // Clarify partial data sources
    if (providerRows.some((r) => r.percentage == null)) {
      note += " · some rows are plan info only (not live remaining)";
    }
  } else if (!connected) {
    note = isOpenCode
      ? "Open an OpenCode session and pick a model (provider/model) to probe balance."
      : "Start an ACP session to receive live usage.";
  } else if (!hasUsageData) {
    note = isGrok
      ? "Context fills in after the first turn. Click refresh for weekly credit usage (_x.ai/billing)."
      : wantsPlanLimits
        ? "Context fills in after the first turn. Click refresh for plan limits."
        : isOpenCode
          ? "Click refresh to query the provider for the selected model."
          : "Context fills in after the first turn.";
  } else if (state?.source) {
    note = `Source: ${state.source}`;
  }

  // The limit rows are gone unless real, so say why — otherwise their absence
  // just looks like a missing feature.
  const hasRateRows = windows.some((w) => w.kind === "rate_limit");
  if (!hasRateRows && wantsPlanLimits && connected) {
    const how =
      agentId === "codex"
        ? "click refresh (runs /status)"
        : isGrok
          ? "click refresh (Grok _x.ai/billing)"
          : "click refresh (runs /usage)";
    note = `${note ? `${note} · ` : ""}Plan limits: ${how}.`;
  }

  return {
    agentId,
    agentLabel: state?.providerLabel
      ? `${agentLabel} · ${state.providerLabel}`
      : agentLabel,
    windows,
    refreshedAt: formatClock(state?.refreshedAt ?? null),
    note,
    cost:
      state?.costAmount != null
        ? { amount: state.costAmount, currency: state.costCurrency ?? "USD" }
        : null,
  };
}
