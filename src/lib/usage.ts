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

export function mergeUsageFromText(
  prev: SessionUsageState | undefined,
  text: string,
  now = new Date()
): SessionUsageState | null {
  const rateWindows = parseCodexStatusRateLimits(text);
  if (rateWindows.length === 0) return null;
  const base = prev ?? emptySessionUsage();
  const windows = { ...base.windows };
  for (const w of rateWindows) windows[w.id] = w;
  return {
    ...base,
    windows,
    refreshedAt: now.toISOString(),
    source: base.source?.includes("status") ? base.source : "agent /status text",
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
  const wantsPlanLimits = agentId === "codex" || agentId === "claude-code" || agentId === "claude";
  const byId = new Map(rateWindows.map((w) => [w.id, w]));
  // Don't duplicate ids already shown from provider probe (Go 5h/weekly).
  for (const row of providerRows) {
    byId.delete(row.id);
  }

  if (wantsPlanLimits) {
    for (const slot of [
      { id: "five-hour", label: "5-hour limit" },
      { id: "weekly", label: "Weekly limit" },
    ] as const) {
      if (providerRows.some((r) => r.id === slot.id)) continue;
      windows.push(
        byId.get(slot.id) ?? {
          id: slot.id,
          label: slot.label,
          percentage: null,
          detail: null,
          kind: "rate_limit",
        }
      );
      byId.delete(slot.id);
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
  } else if (!wantsPlanLimits && !isOpenCode && agentId === "grok") {
    windows.push({
      id: "provider",
      label: "Provider usage",
      percentage: null,
      detail: connected ? "Waiting for agent usage_update" : null,
      kind: "provider",
    });
  }

  // Honest empty-state labels: never imply a live % when we only know plan ceilings.
  for (const w of windows) {
    if (w.percentage == null && !w.detail) {
      if (w.kind === "context") {
        w.detail = connected ? "Waiting for usage_update" : "Not connected";
      } else if (w.kind === "rate_limit") {
        w.detail = "No live remaining % from agent";
      } else if (w.kind === "provider") {
        w.detail = "Unavailable via public API";
      }
    }
  }

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
  } else if (!state?.refreshedAt) {
    note = wantsPlanLimits
      ? "Context fills in after the first turn. Plan limits appear when the adapter reports them (Claude meta / Codex /status)."
      : isOpenCode
        ? "Click refresh to query the provider for the selected model."
        : "Waiting for the agent to report usage_update.";
  } else if (state.source) {
    note = `Source: ${state.source}`;
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
