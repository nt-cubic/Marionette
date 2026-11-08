/**
 * usageDigest — what one turn did, folded from that turn's own events.
 *
 * Runs once at turn/complete over the events after the turn's user_message (the
 * rail already holds them; nothing is reloaded — memory red line). The result is
 * added into the usage ledger (usageLedger.recordTurnUsageInto) so the Token page
 * can show where time went, which tools / commands / skills ran, how many lines
 * changed, and an estimate of the turn's visible input.
 *
 * Honesty rules (docs/ui-language.md「Token 用量」):
 * - Times are wall-clock spans between our own event stamps; a span with no end
 *   is closed by the next event or the turn end, never extended past it.
 * - `est.*` are estimates (text ÷ 4, flat per image) and are labelled as such in
 *   the UI; tool output is clipped upstream, so `toolIo` is a floor, not a total.
 * - Nothing here is invented: a turn with no tool calls has empty tallies.
 */

import type { SessionEvent, TurnStats } from "./types";
import { classifyToolCall, extractCommandText, extractDiffStats } from "./turnActivity";

type ToolCallEvent = Extract<SessionEvent, { type: "tool_call" }>;

/** Count, failures and wall time of one tool / program / skill. */
export type Tally = { n: number; fail: number; ms: number };

export type TimingSplit = {
  /** Send → first streamed chunk (from TurnStats.ttftMs). */
  ttft: number;
  /** Thought cards. */
  think: number;
  /** Reply cards streaming. */
  answer: number;
  /** Tool calls running (commands included). */
  tool: number;
  /** Whatever is left of the turn's wall time (queueing, handshakes, gaps). */
  other: number;
};

export type TurnDigest = {
  timing: TimingSplit;
  /** By tool kind (read / edit / command / search …). */
  tools: Record<string, Tally>;
  /** By program for command tools (git, npm, cargo, pwsh …). */
  cmds: Record<string, Tally>;
  /** Skills and MCP tools, keyed `skill:<name>` / `mcp:<server> · <tool>`. */
  skills: Record<string, Tally>;
  /** Edit / write operations and the diff lines they carried. */
  edits: { ops: number; add: number; del: number };
  /** Estimated tokens of what this turn put into the context that we can see. */
  est: { user: number; attach: number; toolIo: number };
};

/** ≈ tokens of a text (4 chars ≈ 1 token — the same density dsh-context uses). */
export function estimateTokens(text: string | null | undefined): number {
  if (!text) return 0;
  return Math.ceil(text.length / 4);
}

/** Flat per-image estimate: providers bill 0.5–1.6k for a typical screenshot. */
export const IMAGE_TOKENS_EST = 1000;

const FAILED = /^(failed|error|errored|cancelled|canceled|rejected)$/i;

function ms(at: string | undefined): number | null {
  if (!at) return null;
  const v = Date.parse(at);
  return Number.isFinite(v) ? v : null;
}

function bump(map: Record<string, Tally>, key: string, failed: boolean, spent: number): void {
  const cur = map[key] ?? { n: 0, fail: 0, ms: 0 };
  map[key] = { n: cur.n + 1, fail: cur.fail + (failed ? 1 : 0), ms: cur.ms + Math.max(0, spent) };
}

/**
 * Program of a command line: `git status` → git, `C:\x\node.exe a.js` → node,
 * `powershell -Command …` → pwsh, `cmd /c npm i` → npm, `npx tsc` → npx.
 */
export function commandProgram(command: string): string | null {
  // A leading executable path may contain spaces (`C:\Program Files\nodejs\node.exe a.js`).
  const exe = /^"?([A-Za-z]:\\[^"]*?\.(?:exe|cmd|bat))"?(?:\s|$)/i.exec(command.trim());
  if (exe) command = exe[1]!.split("\\").pop()! + command.trim().slice(exe[0].length - 1);
  const tokens = command.trim().split(/\s+/).filter(Boolean);
  let i = 0;
  // Wrapper shells hand the real program to the next token.
  const wrappers = new Set(["cmd", "cmd.exe", "sh", "bash", "zsh", "/bin/sh", "/bin/bash", "env", "sudo", "time"]);
  while (i < tokens.length) {
    const raw = tokens[i]!.replace(/^["']|["']$/g, "");
    const base = raw.split(/[\\/]/).pop()!.toLowerCase().replace(/\.(exe|cmd|bat|ps1)$/, "");
    if (/^[A-Z_][A-Z0-9_]*=/.test(raw)) {
      i += 1; // FOO=bar prefix
      continue;
    }
    if (wrappers.has(base) || wrappers.has(raw.toLowerCase())) {
      i += 1;
      while (i < tokens.length && /^[-/]/.test(tokens[i]!)) i += 1; // /c, -c, -lc
      continue;
    }
    if (base === "powershell" || base === "pwsh") return "pwsh";
    if (!base || /^[-/]/.test(base)) return null;
    return base;
  }
  return null;
}

/** `mcp__server__tool` / `mcp:server/tool` → `server · tool`; null when not MCP. */
export function mcpToolKey(name: string | undefined): string | null {
  if (!name) return null;
  const a = /^mcp__([^_]+(?:_[^_]+)*?)__(.+)$/.exec(name);
  if (a) return `${a[1]} · ${a[2]}`;
  const b = /^mcp[:/](.+?)[/:](.+)$/i.exec(name);
  if (b) return `${b[1]} · ${b[2]}`;
  return null;
}

/** Skill name when the call loads a skill (`Skill` tool with `{skill|name}` input). */
export function skillName(event: ToolCallEvent): string | null {
  const n = (event.toolName || "").toLowerCase();
  if (n !== "skill" && n !== "load_skill" && n !== "use_skill") return null;
  try {
    const parsed = JSON.parse(event.input ?? "") as Record<string, unknown>;
    for (const key of ["skill", "name", "command"]) {
      const v = parsed[key];
      if (typeof v === "string" && v.trim()) return v.trim();
    }
  } catch {
    /* clipped input */
  }
  const fromTitle = (event.title ?? "").replace(/^skill[:\s]*/i, "").trim();
  return fromTitle || "skill";
}

/** Events of the turn that just ended: after the last user_message of the session. */
export function currentTurnEvents(events: readonly SessionEvent[], sessionId: string): SessionEvent[] {
  let start = -1;
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i]!;
    if (e.sessionId === sessionId && e.type === "user_message") {
      start = i;
      break;
    }
  }
  const out: SessionEvent[] = [];
  for (let i = Math.max(0, start); i < events.length; i++) {
    const e = events[i]!;
    if (e.sessionId === sessionId) out.push(e);
  }
  return out;
}

export function digestTurn(
  events: readonly SessionEvent[],
  sessionId: string,
  opts: { stats: TurnStats | null; sentAt: number | null; endedAt: number },
): TurnDigest {
  const turn = currentTurnEvents(events, sessionId);
  const digest: TurnDigest = {
    timing: { ttft: 0, think: 0, answer: 0, tool: 0, other: 0 },
    tools: {},
    cmds: {},
    skills: {},
    edits: { ops: 0, add: 0, del: 0 },
    est: { user: 0, attach: 0, toolIo: 0 },
  };
  const end = opts.endedAt;
  const user = turn[0]?.type === "user_message" ? turn[0] : null;
  if (user) {
    digest.est.user = estimateTokens(user.text);
    digest.est.attach = (user.attachments?.length ?? 0) * IMAGE_TOKENS_EST;
  }

  // Each card's span runs to its own end stamp, else to the next card, else to the turn end.
  const body = user ? turn.slice(1) : turn;
  for (let i = 0; i < body.length; i++) {
    const e = body[i]!;
    const start = ms(e.createdAt);
    if (start == null) continue;
    const nextStart = ms(body[i + 1]?.createdAt) ?? end;
    const own =
      e.type === "tool_call" ? ms(e.completedAt) : e.type === "thought" || e.type === "assistant_message" ? ms(e.endedAt) : null;
    const stop = Math.min(end, own ?? nextStart);
    const span = Math.max(0, stop - start);
    if (e.type === "thought") digest.timing.think += span;
    else if (e.type === "assistant_message") digest.timing.answer += span;
    else if (e.type === "tool_call") {
      digest.timing.tool += span;
      const failed = FAILED.test(e.status ?? "");
      const kind = classifyToolCall(e);
      bump(digest.tools, kind, failed, span);
      digest.est.toolIo += estimateTokens(e.input) + estimateTokens(e.detail);
      if (kind === "command") {
        const program = commandProgram(extractCommandText(e));
        if (program) bump(digest.cmds, program, failed, span);
      }
      const mcp = mcpToolKey(e.toolName);
      const skill = mcp ? null : skillName(e);
      if (mcp) bump(digest.skills, `mcp:${mcp}`, failed, span);
      else if (skill) bump(digest.skills, `skill:${skill}`, failed, span);
      if (kind === "edit" || kind === "write") {
        digest.edits.ops += 1;
        const diff = extractDiffStats(e.detail);
        if (diff) {
          digest.edits.add += diff.add;
          digest.edits.del += diff.del;
        }
      }
    }
  }

  const ttft = Math.max(0, opts.stats?.ttftMs ?? 0);
  digest.timing.ttft = ttft;
  const wall = opts.sentAt != null ? Math.max(0, end - opts.sentAt) : 0;
  const counted = ttft + digest.timing.think + digest.timing.answer + digest.timing.tool;
  // Spans can overlap (parallel tools): never report a negative remainder.
  digest.timing.other = Math.max(0, wall - counted);
  return digest;
}
