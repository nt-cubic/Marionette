import { cachedCapabilitiesFor } from "./capabilityCache";
import type { AgentConfig, ModelDef } from "./types";

/** Aliases → canonical agent id. */
export const DELEGATE_ALIASES: Record<string, string> = {
  oc: "opencode",
  opencode: "opencode",
  cc: "claude-code",
  claude: "claude-code",
  "claude-code": "claude-code",
  grok: "grok-build",
  "grok-build": "grok-build",
  gpt: "codex",
  codex: "codex",
  omp: "omp",
  "oh-my-pi": "omp",
};

export type DelegateCandidate = {
  agentId: string;
  agentLabel: string;
  /** Optional model id under this agent. */
  modelId?: string;
  modelLabel?: string;
  installed: boolean;
  /** True when model came from recent-usage ranking. */
  recent?: boolean;
};

export type DelegateToken = {
  /** Inclusive start of `@…` in draft. */
  start: number;
  /** Exclusive end of the agent[/model] token (not the task text). */
  end: number;
  /** Raw token after `@` (may be partial, e.g. `op` or `opencode/deep`). */
  raw: string;
  agentQuery: string;
  modelQuery: string | null;
  /** True when still editing the token (no trailing space yet after token). */
  completing: boolean;
};

export type ParsedDelegate = {
  agentId: string;
  modelId?: string;
  /** Task text without the @prefix. */
  prompt: string;
  /** Full original line for the parent card. */
  rawLine: string;
};

/**
 * Active `@` token at the caret — **line-start only** (`^\s*@`).
 * `@@` at line start is escape (literal @) and returns null.
 */
export function delegateQueryAtCursor(
  draft: string,
  cursor: number,
): DelegateToken | null {
  if (!draft.includes("@")) return null;
  const before = draft.slice(0, Math.max(0, cursor));
  const lineStart = before.lastIndexOf("\n") + 1;
  const line = before.slice(lineStart);
  // Escape: line-start `@@`
  if (/^\s*@@/.test(line)) return null;
  // Completing: `@token` or `@agent/` or `@agent/model` with optional partial, no space after token start... 
  // Match from line start: optional spaces, @, then non-space run (agent[/model partial]).
  const m = line.match(/^(\s*)@([^\s]*)$/);
  if (!m) return null;
  const lead = m[1].length;
  const raw = m[2];
  const start = lineStart + lead;
  const slash = raw.indexOf("/");
  const agentQuery = (slash >= 0 ? raw.slice(0, slash) : raw).toLowerCase();
  const modelQuery = slash >= 0 ? raw.slice(slash + 1).toLowerCase() : null;
  return {
    start,
    end: cursor,
    raw,
    agentQuery,
    modelQuery,
    completing: true,
  };
}

export function resolveAgentId(token: string): string | null {
  const key = token.trim().toLowerCase();
  if (!key) return null;
  if (DELEGATE_ALIASES[key]) return DELEGATE_ALIASES[key];
  // Unique prefix match on alias keys + values
  const ids = new Set(Object.values(DELEGATE_ALIASES));
  const hits = [...ids].filter((id) => id.startsWith(key) || key.startsWith(id));
  const aliasHits = Object.entries(DELEGATE_ALIASES)
    .filter(([alias]) => alias.startsWith(key))
    .map(([, id]) => id);
  const all = [...new Set([...hits, ...aliasHits])];
  if (all.length === 1) return all[0];
  if (ids.has(key)) return key;
  return null;
}

function matchModel(
  agentId: string,
  modelQuery: string | null | undefined,
): { modelId?: string; modelLabel?: string; unique: boolean } {
  if (!modelQuery) return { unique: true };
  const caps = cachedCapabilitiesFor(agentId);
  const models = caps?.models ?? [];
  if (models.length === 0) return { unique: true }; // offline unknown — allow bare agent
  const q = modelQuery.toLowerCase();
  const hits = models.filter(
    (m) =>
      m.id.toLowerCase() === q ||
      m.id.toLowerCase().startsWith(q) ||
      (m.label && m.label.toLowerCase().includes(q)),
  );
  if (hits.length === 1) {
    return { modelId: hits[0].id, modelLabel: hits[0].label, unique: true };
  }
  if (hits.length === 0) {
    // exact id allow even if not cached
    return { modelId: modelQuery, unique: true };
  }
  return { unique: false };
}

/**
 * Parse a full send line as a delegate command.
 * Returns null if not a delegate (including `@src/foo` with no agent match).
 */
export function parseDelegateLine(
  text: string,
  knownAgentIds: string[],
): ParsedDelegate | null {
  const rawLine = text.replace(/\r\n/g, "\n").trim();
  if (!rawLine) return null;
  // Only first line is the directive; rest can be task body
  const lines = rawLine.split("\n");
  const first = lines[0];
  if (/^\s*@@/.test(first)) return null;
  const m = first.match(/^\s*@(\S+)(?:\s+([\s\S]*))?$/);
  if (!m) return null;

  const head = m[1];
  let rest = (m[2] ?? "").trim();
  const restLines = lines.slice(1).join("\n").trim();
  if (restLines) rest = rest ? `${rest}\n${restLines}` : restLines;

  // Form: agent/model or agent
  let agentToken = head;
  let modelFromSlash: string | undefined;
  if (head.includes("/")) {
    const i = head.indexOf("/");
    agentToken = head.slice(0, i);
    modelFromSlash = head.slice(i + 1) || undefined;
  }

  let agentId = resolveAgentId(agentToken);
  // Also accept raw known ids not in alias table
  if (!agentId) {
    const key = agentToken.toLowerCase();
    const hit = knownAgentIds.find((id) => id.toLowerCase() === key || id.toLowerCase().startsWith(key));
    if (hit && knownAgentIds.filter((id) => id.toLowerCase().startsWith(key)).length === 1) {
      agentId = hit;
    }
  }
  if (!agentId) return null;
  if (!knownAgentIds.includes(agentId) && !Object.values(DELEGATE_ALIASES).includes(agentId)) {
    // still allow if resolve succeeded via alias
    if (!Object.values(DELEGATE_ALIASES).includes(agentId)) return null;
  }

  let modelId = modelFromSlash;
  // Loose form: @agent model task — second token is model if unique prefix match
  if (!modelId && rest) {
    const parts = rest.split(/\s+/);
    if (parts.length >= 2) {
      const maybeModel = parts[0];
      const matched = matchModel(agentId, maybeModel);
      if (matched.unique && matched.modelId && maybeModel.length >= 2) {
        const caps = cachedCapabilitiesFor(agentId);
        const inCatalog =
          caps?.models.some(
            (m) =>
              m.id.toLowerCase() === maybeModel.toLowerCase() ||
              m.id.toLowerCase().startsWith(maybeModel.toLowerCase()),
          ) ?? false;
        if (inCatalog) {
          modelId = matched.modelId;
          rest = parts.slice(1).join(" ");
        }
      }
    }
  } else if (modelId) {
    const matched = matchModel(agentId, modelId);
    if (matched.modelId) modelId = matched.modelId;
  }

  if (!rest.trim()) return null; // need a task

  return {
    agentId,
    modelId,
    prompt: rest.trim(),
    rawLine,
  };
}

/** Insert `@agent/` or `@agent/model ` into the draft. */
export function applyDelegateCandidate(
  draft: string,
  start: number,
  end: number,
  candidate: DelegateCandidate,
): string {
  const token = candidate.modelId
    ? `@${candidate.agentId}/${candidate.modelId} `
    : `@${candidate.agentId}/`;
  // If no model, leave trailing `/` for model completion; if agent-only with no models, use space
  const insert =
    candidate.modelId || candidate.modelLabel
      ? `@${candidate.agentId}/${candidate.modelId ?? ""} `.replace(/\/\s$/, "/ ")
      : `@${candidate.agentId} `;
  // Prefer slash form when models may exist
  const caps = cachedCapabilitiesFor(candidate.agentId);
  const hasModels = (caps?.models.length ?? 0) > 0;
  const finalInsert = candidate.modelId
    ? `@${candidate.agentId}/${candidate.modelId} `
    : hasModels
      ? `@${candidate.agentId}/`
      : `@${candidate.agentId} `;
  void token;
  void insert;
  return draft.slice(0, start) + finalInsert + draft.slice(end);
}

export function filterDelegateCandidates(
  agents: AgentConfig[],
  installedIds: Set<string>,
  token: DelegateToken,
  recentModelIds: string[] = [],
): DelegateCandidate[] {
  const agentIds = agents.map((a) => a.id);
  const q = token.agentQuery;

  // Build agent list: filter by query on id/label/aliases
  let matchedAgents = agents.filter((a) => {
    if (!q) return true;
    if (a.id.toLowerCase().startsWith(q) || a.id.toLowerCase().includes(q)) return true;
    if (a.label.toLowerCase().includes(q)) return true;
    return Object.entries(DELEGATE_ALIASES).some(
      ([alias, id]) => id === a.id && alias.startsWith(q),
    );
  });

  // If agent is fully resolved and we're on model query, focus that agent
  const resolved = resolveAgentId(q) || (agentIds.includes(q) ? q : null);
  if (token.modelQuery !== null && resolved) {
    matchedAgents = agents.filter((a) => a.id === resolved);
  }

  const out: DelegateCandidate[] = [];
  for (const agent of matchedAgents.slice(0, 8)) {
    const installed = installedIds.has(agent.id);
    const caps = cachedCapabilitiesFor(agent.id);
    const models: ModelDef[] = caps?.models ?? [];

    if (token.modelQuery !== null && models.length > 0) {
      const mq = token.modelQuery;
      let modelHits = models.filter(
        (m) =>
          !mq ||
          m.id.toLowerCase().startsWith(mq) ||
          m.id.toLowerCase().includes(mq) ||
          (m.label && m.label.toLowerCase().includes(mq)),
      );
      // Rank recent first
      modelHits = [...modelHits].sort((a, b) => {
        const ai = recentModelIds.indexOf(a.id);
        const bi = recentModelIds.indexOf(b.id);
        if (ai === -1 && bi === -1) return 0;
        if (ai === -1) return 1;
        if (bi === -1) return -1;
        return ai - bi;
      });
      for (const m of modelHits.slice(0, 6)) {
        out.push({
          agentId: agent.id,
          agentLabel: agent.label,
          modelId: m.id,
          modelLabel: m.label,
          installed,
          recent: recentModelIds.includes(m.id),
        });
      }
      if (modelHits.length === 0) {
        out.push({
          agentId: agent.id,
          agentLabel: agent.label,
          installed,
        });
      }
    } else {
      // Agent row + top recent model as nested hint
      out.push({
        agentId: agent.id,
        agentLabel: agent.label,
        installed,
      });
      if (models.length > 0 && !q) {
        // skip nested when browsing all
      } else if (models.length > 0 && resolved === agent.id) {
        for (const m of models.slice(0, 4)) {
          out.push({
            agentId: agent.id,
            agentLabel: agent.label,
            modelId: m.id,
            modelLabel: m.label,
            installed,
            recent: recentModelIds[0] === m.id,
          });
        }
      }
    }
  }
  return out.slice(0, 12);
}

export function formatSubtaskDuration(ms: number): string {
  if (ms < 1000) return "0:00";
  const s = Math.floor(ms / 1000);
  const m = Math.floor(s / 60);
  const r = s % 60;
  if (m < 60) return `${m}:${r.toString().padStart(2, "0")}`;
  const h = Math.floor(m / 60);
  return `${h}h${m % 60}m`;
}
