/**
 * P0-1 report: same-session, five-hop comparison for delegated-worker events.
 *
 * Reads the structured `subagent-observe` lines the app writes to the developer
 * diary, plus the tee file and the transcript file, and prints one table per
 * session so the hop that loses events is visible instead of guessed at.
 *
 * Hops:
 *   1. agent emitted          (external baseline; supplied manually, never invented)
 *   2. ACP parsed             (acp.rs `session/update` by kind)
 *   3. tee                    (teed queued/written/dropped + file scan)
 *   4. transcript             (file scan of `<sid>.jsonl` + write-batch lines)
 *   5. replay                 (backfillChiefTee applied / skipped / aborted)
 *
 * Usage:
 *   npx tsx scripts/report-subagent-pipeline.mts
 *   npx tsx scripts/report-subagent-pipeline.mts --session session_123
 *   npx tsx scripts/report-subagent-pipeline.mts --baseline session_123=492
 *   npx tsx scripts/report-subagent-pipeline.mts --baseline-file baseline.json
 *   npx tsx scripts/report-subagent-pipeline.mts --json
 *   npx tsx scripts/report-subagent-pipeline.mts --self-test
 *
 * Measurement only: this script never mutates app state or event data.
 */
import { createReadStream, existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

export type ParsedLogLine = {
  ts: number;
  level: string;
  source: string;
  sessionId: string;
  summary: string;
  detail: string;
};

export type KindCounts = Record<string, number>;

export type Stage2 = {
  total: number;
  kinds: KindCounts;
  reason: string;
  ts: number;
};

export type Stage3 = {
  enqueued: number;
  written: number;
  dropped: number;
  fileLines: number;
  ts: number;
};

export type Stage4Batches = {
  writes: number;
  batches: number;
  kinds: KindCounts;
  lastMode: string;
  lastTs: number;
};

export type Stage5 = {
  attempts: number;
  latest: Record<string, unknown> | null;
  outcomes: Record<string, number>;
  lastTs: number;
};

export type SessionStages = {
  stage2?: Stage2;
  stage3?: Stage3;
  stage3Seed?: number;
  stage4?: Stage4Batches;
  stage5: Stage5;
};

export type Baseline = {
  total?: number;
  toolCalls?: number;
};

export type FileScan = {
  path: string | null;
  totals: KindCounts;
  lines: number;
  scanError?: string;
};

const DEFAULT_LOG_FILES = ["dev.log.3", "dev.log.2", "dev.log.1", "dev.log"];

/** Parse one dev.log line: `[ts] [level] [source] session=<id> <summary> | <detail>`. */
export function parseLogLine(raw: string): ParsedLogLine | null {
  const match = /^\[(\d+)\] \[([^\]]*)\] \[([^\]]*)\] session=(\S+)\s?(.*)$/.exec(raw);
  if (!match) return null;
  const [, ts, level, source, sessionId, rest] = match;
  const sep = rest.indexOf(" | ");
  const summary = sep >= 0 ? rest.slice(0, sep) : rest;
  const detail = sep >= 0 ? rest.slice(sep + 3) : "";
  return {
    ts: Number(ts),
    level,
    source,
    sessionId,
    summary,
    detail,
  };
}

export function parseLogLines(text: string): ParsedLogLine[] {
  const out: ParsedLogLine[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const parsed = parseLogLine(raw);
    if (parsed) out.push(parsed);
  }
  return out;
}

function parseDetail(detail: string): Record<string, unknown> | null {
  if (!detail) return null;
  try {
    const value = JSON.parse(detail);
    return value && typeof value === "object" ? (value as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

function asNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function asKinds(value: unknown): KindCounts {
  if (!value || typeof value !== "object") return {};
  const out: KindCounts = {};
  for (const [key, count] of Object.entries(value as Record<string, unknown>)) {
    const n = asNumber(count);
    if (n !== undefined) out[key] = n;
  }
  return out;
}

function addKinds(target: KindCounts, source: KindCounts): void {
  for (const [key, count] of Object.entries(source)) {
    target[key] = (target[key] ?? 0) + count;
  }
}

/** Fold the `subagent-observe` diary lines into per-session stage snapshots. */
export function collectStages(lines: ParsedLogLine[]): Map<string, SessionStages> {
  const sessions = new Map<string, SessionStages>();
  const ensure = (sessionId: string): SessionStages => {
    let entry = sessions.get(sessionId);
    if (!entry) {
      entry = { stage5: { attempts: 0, latest: null, outcomes: {}, lastTs: 0 } };
      sessions.set(sessionId, entry);
    }
    return entry;
  };

  for (const line of lines) {
    if (line.source !== "subagent-observe") continue;
    const detail = parseDetail(line.detail);
    if (!detail) continue;
    const stage = asNumber(detail.stage);
    const hop = asString(detail.hop);
    const entry = ensure(line.sessionId);
    if (stage === 2 && hop === "acp_parse") {
      entry.stage2 = {
        total: asNumber(detail.total) ?? 0,
        kinds: asKinds(detail.kinds),
        reason: asString(detail.reason) ?? "",
        ts: line.ts,
      };
    } else if (stage === 3 && hop === "tee") {
      entry.stage3 = {
        enqueued: asNumber(detail.enqueued) ?? 0,
        written: asNumber(detail.written) ?? 0,
        dropped: asNumber(detail.dropped) ?? 0,
        fileLines: asNumber(detail.fileLines) ?? 0,
        ts: line.ts,
      };
    } else if (stage === 3 && hop === "tee_register") {
      entry.stage3Seed = asNumber(detail.seedLines) ?? 0;
    } else if (stage === 4 && hop === "transcript") {
      const batch = asNumber(detail.batch) ?? 0;
      const entry4 = entry.stage4 ?? {
        writes: 0,
        batches: 0,
        kinds: {},
        lastMode: "",
        lastTs: 0,
      };
      entry4.writes += 1;
      entry4.batches += batch;
      addKinds(entry4.kinds, asKinds(detail.kinds));
      entry4.lastMode = asString(detail.mode) ?? entry4.lastMode;
      entry4.lastTs = line.ts;
      entry.stage4 = entry4;
    } else if (stage === 5 && hop === "replay") {
      entry.stage5.attempts += 1;
      entry.stage5.latest = detail;
      entry.stage5.lastTs = line.ts;
      const outcome = asString(detail.outcome) ?? "unknown";
      entry.stage5.outcomes[outcome] = (entry.stage5.outcomes[outcome] ?? 0) + 1;
    }
  }
  return sessions;
}

/** Scan a tee JSONL line into the update-kind distribution. */
export function foldTeeLine(line: string, totals: KindCounts): boolean {
  if (!line.trim()) return false;
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(line) as Record<string, unknown>;
  } catch {
    return false;
  }
  const method = asString(parsed.method) ?? "";
  const data = parsed.data as { params?: { update?: { sessionUpdate?: unknown } } } | undefined;
  const kind =
    method === "session/update" ? asString(data?.params?.update?.sessionUpdate) : undefined;
  const key = kind ?? (method ? `event:${method}` : "event:unknown");
  totals[key] = (totals[key] ?? 0) + 1;
  return true;
}

/** Scan a transcript JSONL line into the event-type distribution. */
export function foldTranscriptLine(line: string, totals: KindCounts): boolean {
  if (!line.trim()) return false;
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(line) as Record<string, unknown>;
  } catch {
    return false;
  }
  const type = asString(parsed.type) ?? "unknown";
  totals[type] = (totals[type] ?? 0) + 1;
  return true;
}

export function summarizeKindCounts(lines: string[], fold: (line: string, totals: KindCounts) => boolean): {
  totals: KindCounts;
  lines: number;
} {
  const totals: KindCounts = {};
  let used = 0;
  for (const line of lines) {
    if (fold(line, totals)) used += 1;
  }
  return { totals, lines: used };
}

function toolCall(kinds: KindCounts | undefined): number | undefined {
  if (!kinds) return undefined;
  if (kinds.tool_call !== undefined) return kinds.tool_call;
  return undefined;
}

function totalOfKinds(kinds: KindCounts): number {
  let total = 0;
  for (const count of Object.values(kinds)) total += count;
  return total;
}

function stage5Value(stage5: Stage5, key: string): number {
  return asNumber(stage5.latest?.[key]) ?? 0;
}

function stage5Role(stage5: Stage5, role: string): number {
  const roles = stage5.latest?.appliedByRole;
  if (!roles || typeof roles !== "object") return 0;
  return asNumber((roles as Record<string, unknown>)[role]) ?? 0;
}

export type SessionReport = {
  sessionId: string;
  stages: SessionStages;
  tee: FileScan;
  transcript: FileScan;
  baseline?: Baseline;
  notes: string[];
  hopToolCalls: Array<{ hop: string; toolCalls?: number; total?: number }>;
};

/** Build the verdict notes that point at the first hop whose count collapses. */
export function verdictFor(
  stages: SessionStages,
  tee: FileScan,
  transcript: FileScan,
  baseline?: Baseline,
): string[] {
  const notes: string[] = [];
  const hops: Array<{ hop: string; toolCalls?: number }> = [
    { hop: "1 agent", toolCalls: baseline?.toolCalls },
    { hop: "2 acp", toolCalls: toolCall(stages.stage2?.kinds) },
    { hop: "3 tee", toolCalls: toolCall(tee.totals) },
    { hop: "4 transcript", toolCalls: toolCall(transcript.totals) },
    { hop: "5 replay", toolCalls: stages.stage5.attempts > 0 ? stage5Role(stages.stage5, "tool") : undefined },
  ];

  // Stage 5 is a replay of the *uncovered* span, so its applied count is only
  // comparable when the transcript had nothing yet. Only flag the hop if a
  // meaningful tool_call count collapses to a sliver.
  for (let i = 1; i < hops.length; i += 1) {
    const prev = hops[i - 1].toolCalls;
    const next = hops[i].toolCalls;
    if (prev === undefined || next === undefined) continue;
    if (prev >= 10 && next === 0) {
      notes.push(`BREAK at ${hops[i].hop}: tool_call ${prev} -> 0`);
    } else if (prev >= 10 && next <= Math.floor(prev * 0.2)) {
      notes.push(`SUSPECT ${hops[i].hop}: tool_call ${prev} -> ${next}`);
    }
  }

  if ((toolCall(tee.totals) ?? 0) > 0 && transcript.lines === 0) {
    const replayApplied = stage5Role(stages.stage5, "tool");
    if (stages.stage5.attempts === 0) {
      notes.push("stage 4 transcript is empty and stage 5 was never observed (open the worker session)");
    } else if (replayApplied === 0) {
      notes.push("transcript is empty and replay applied no tool calls");
    } else {
      notes.push("transcript is empty; this replay is the only path (rails will flush after the debounce)");
    }
  }
  if (stages.stage5.attempts > 0) {
    const latestOutcome = asString(stages.stage5.latest?.outcome) ?? "unknown";
    const uncovered = stage5Value(stages.stage5, "uncovered");
    if (latestOutcome === "applied" && uncovered >= 10 && stage5Role(stages.stage5, "tool") === 0) {
      notes.push("SUSPECT stage 5 replay: uncovered span had events but no tool call was applied");
    }
    if (stage5Value(stages.stage5, "baselineAborts") > 0) {
      notes.push("stage 5 aborted on rail growth past baseline (cursor not advanced; reopen retries)");
    }
    if (stage5Value(stages.stage5, "dedupeSkips") > 0) {
      notes.push(
        `stage 5 dedupe skips: ${stage5Value(stages.stage5, "dedupeSkips")} (merged into existing cards)`,
      );
    }
  }
  return notes;
}

export function buildSessionReport(
  sessionId: string,
  stages: SessionStages,
  tee: FileScan,
  transcript: FileScan,
  baseline?: Baseline,
): SessionReport {
  const hopToolCalls: SessionReport["hopToolCalls"] = [
    { hop: "1 agent", toolCalls: baseline?.toolCalls, total: baseline?.total },
    { hop: "2 acp", toolCalls: toolCall(stages.stage2?.kinds), total: stages.stage2?.total },
    { hop: "3 tee", toolCalls: toolCall(tee.totals), total: totalOfKinds(tee.totals) },
    { hop: "4 transcript", toolCalls: toolCall(transcript.totals), total: transcript.lines },
    {
      hop: "5 replay",
      toolCalls: stages.stage5.attempts > 0 ? stage5Role(stages.stage5, "tool") : undefined,
      total: stages.stage5.attempts > 0 ? stage5Value(stages.stage5, "applied") : undefined,
    },
  ];
  return {
    sessionId,
    stages,
    tee,
    transcript,
    baseline,
    notes: verdictFor(stages, tee, transcript, baseline),
    hopToolCalls,
  };
}

function fmt(value: number | undefined): string {
  return value === undefined ? "-" : String(value);
}

function kindPairs(kinds: KindCounts | undefined, keys: string[]): string {
  if (!kinds) return "-";
  const parts: string[] = [];
  for (const key of keys) {
    const count = kinds[key];
    if (count !== undefined) parts.push(`${key}=${count}`);
  }
  return parts.length ? parts.join(" ") : "-";
}

export function renderSession(report: SessionReport): string {
  const { sessionId, stages, tee, transcript, baseline } = report;
  const out: string[] = [];
  out.push(`== session ${sessionId} ==`);
  out.push(`  tee file       : ${tee.path ?? "(not found)"}${tee.scanError ? ` (${tee.scanError})` : ""}`);
  out.push(
    `  transcript     : ${transcript.path ?? "(not found)"}${
      transcript.scanError ? ` (${transcript.scanError})` : ""
    }`,
  );
  out.push(
    `  hop 1 agent    : total=${fmt(baseline?.total)} tool_call=${fmt(baseline?.toolCalls)}  (external baseline)`,
  );
  out.push(
    `  hop 2 acp      : total=${fmt(stages.stage2?.total)}  ${kindPairs(stages.stage2?.kinds, [
      "tool_call",
      "tool_call_update",
      "agent_message_chunk",
      "agent_thought_chunk",
      "user_message_chunk",
      "other",
    ])}  [${stages.stage2?.reason ?? "not observed"}]`,
  );
  out.push(
    `  hop 3 tee      : enqueued=${fmt(stages.stage3?.enqueued)} written=${fmt(
      stages.stage3?.written,
    )} dropped=${fmt(stages.stage3?.dropped)} fileLines=${fmt(stages.stage3?.fileLines)}  seedLines=${fmt(
      stages.stage3Seed,
    )}`,
  );
  out.push(
    `  hop 3 file scan: lines=${tee.lines}  ${kindPairs(tee.totals, [
      "tool_call",
      "tool_call_update",
      "agent_message_chunk",
      "agent_thought_chunk",
      "user_message_chunk",
    ])}`,
  );
  out.push(
    `  hop 4 transcript: lines=${transcript.lines}  ${kindPairs(transcript.totals, [
      "tool_call",
      "assistant_message",
      "thought",
      "user_message",
      "notice",
    ])}`,
  );
  out.push(
    `  hop 4 writes   : batches=${fmt(stages.stage4?.writes)} events=${fmt(
      stages.stage4?.batches,
    )} lastMode=${stages.stage4?.lastMode || "-"}`,
  );
  if (stages.stage5.attempts > 0) {
    out.push(
      `  hop 5 replay   : attempts=${stages.stage5.attempts} outcomes=${JSON.stringify(
        stages.stage5.outcomes,
      )} latest=${
        asString(stages.stage5.latest?.outcome) ?? "unknown"
      } applied=${stage5Value(stages.stage5, "applied")} dedupeSkips=${stage5Value(
        stages.stage5,
        "dedupeSkips",
      )}`,
    );
    out.push(
      `  hop 5 filters  : totalTee=${stage5Value(stages.stage5, "totalTee")} afterCursor=${stage5Value(
        stages.stage5,
        "afterCursor",
      )} cursorSkips=${stage5Value(stages.stage5, "cursorSkips")} uncovered=${stage5Value(
        stages.stage5,
        "uncovered",
      )} uncoveredFiltered=${stage5Value(stages.stage5, "uncoveredFiltered")} baselineAborts=${stage5Value(
        stages.stage5,
        "baselineAborts",
      )}`,
    );
    out.push(
      `  hop 5 roles    : tool=${stage5Role(stages.stage5, "tool")} assistant=${stage5Role(
        stages.stage5,
        "assistant",
      )} thought=${stage5Role(stages.stage5, "thought")} system=${stage5Role(
        stages.stage5,
        "system",
      )} userEchoCards=${stage5Value(stages.stage5, "userEchoCards")}`,
    );
  } else {
    out.push("  hop 5 replay   : not observed (open the worker session so backfillChiefTee runs)");
  }
  out.push(`  tool_call by hop: ${report.hopToolCalls.map((h) => `${h.hop}=${fmt(h.toolCalls)}`).join(" -> ")}`);
  if (report.notes.length) {
    for (const note of report.notes) {
      const marker = note.startsWith("BREAK") || note.startsWith("SUSPECT") ? "!" : "i";
      out.push(`  ${marker} ${note}`);
    }
  } else {
    out.push("  verdict        : no hop collapses tool_call counts");
  }
  return out.join("\n");
}

function parseBaselineValue(value: unknown): Baseline | undefined {
  const n = asNumber(value);
  if (n !== undefined) return { total: n };
  if (!value || typeof value !== "object") return undefined;
  const obj = value as Record<string, unknown>;
  const total = asNumber(obj.total);
  const toolCalls = asNumber(obj.toolCalls) ?? asNumber(obj.tool_call);
  if (total === undefined && toolCalls === undefined) return undefined;
  return { total, toolCalls };
}

/** Normalize the `--baseline-file` payload. */
export function parseBaselineFile(text: string): Record<string, Baseline> {
  const out: Record<string, Baseline> = {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return out;
  }
  if (Array.isArray(parsed)) {
    for (const row of parsed) {
      if (!row || typeof row !== "object") continue;
      const sessionId = asString((row as Record<string, unknown>).sessionId);
      if (!sessionId) continue;
      const baseline = parseBaselineValue(row);
      if (baseline) out[sessionId] = baseline;
    }
    return out;
  }
  if (parsed && typeof parsed === "object") {
    for (const [sessionId, value] of Object.entries(parsed as Record<string, unknown>)) {
      const baseline = parseBaselineValue(value);
      if (baseline) out[sessionId] = baseline;
    }
  }
  return out;
}

async function readLines(filePath: string, limit = 2_000_000): Promise<string[] | null> {
  if (!existsSync(filePath)) return null;
  const lines: string[] = [];
  const stream = createReadStream(filePath, { encoding: "utf8" });
  const reader = createInterface({ input: stream, crlfDelay: Infinity });
  try {
    for await (const line of reader) {
      lines.push(line);
      if (lines.length >= limit) break;
    }
  } finally {
    reader.close();
    stream.close();
  }
  return lines;
}

function defaultHome(): string {
  return process.env.USERPROFILE || homedir();
}

function readProjects(home: string, override?: string): Array<{ rootPath?: string }> {
  const file = override ?? path.join(home, ".marionette", "projects.json");
  try {
    const raw = readFileSync(file, "utf8");
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/** Locate `<sid>.jsonl` in chat transcripts or any project's `.marionette`. */
export function findTranscriptPath(
  sessionId: string,
  home: string,
  projects: Array<{ rootPath?: string }>,
): string | null {
  const candidates = [
    path.join(home, ".marionette", "chats", "transcripts", `${sessionId}.jsonl`),
    ...projects
      .map((project) => project.rootPath)
      .filter((root): root is string => typeof root === "string" && root.length > 0)
      .map((root) => path.join(root, ".marionette", "transcripts", `${sessionId}.jsonl`)),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

export function findTeePath(sessionId: string, home: string, override?: string): string {
  return override ?? path.join(home, ".marionette", "chief", "sessions", `${sessionId}.events.jsonl`);
}

type CliOptions = {
  sessions: string[];
  logs: string[];
  home: string;
  baselineFile?: string;
  baselines: Record<string, Baseline>;
  json: boolean;
  selfTest: boolean;
  maxSessions: number;
};

function parseArgs(argv: string[]): CliOptions {
  const options: CliOptions = {
    sessions: [],
    logs: [],
    home: defaultHome(),
    baselines: {},
    json: false,
    selfTest: false,
    maxSessions: 25,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--session") options.sessions.push(argv[++i] ?? "");
    else if (arg === "--log") options.logs.push(argv[++i] ?? "");
    else if (arg === "--home") options.home = argv[++i] ?? options.home;
    else if (arg === "--baseline-file") options.baselineFile = argv[++i];
    else if (arg === "--json") options.json = true;
    else if (arg === "--self-test") options.selfTest = true;
    else if (arg === "--max-sessions") options.maxSessions = Number(argv[++i] ?? 25) || 25;
    else if (arg === "--baseline") {
      const spec = argv[++i] ?? "";
      const eq = spec.lastIndexOf("=");
      if (eq > 0) {
        const sessionId = spec.slice(0, eq);
        const value = Number(spec.slice(eq + 1));
        if (Number.isFinite(value)) options.baselines[sessionId] = { total: value };
      }
    } else if (arg === "--help" || arg === "-h") {
      options.selfTest = false;
      printHelp();
      process.exit(0);
    }
  }
  return options;
}

function printHelp(): void {
  console.log(
    [
      "report-subagent-pipeline.mts",
      "",
      "  --session <id>            limit to one session (repeatable)",
      "  --log <path>              extra dev.log to read (repeatable; defaults to ~/.marionette/logs/dev.log*)",
      "  --home <dir>              override the home dir (default: %USERPROFILE%)",
      "  --baseline <sid>=<total>  stage 1 external total (repeatable)",
      "  --baseline-file <json>    map or array with {sessionId, total?, toolCalls?}",
      "  --json                    machine-readable output",
      "  --max-sessions <n>        cap output (default 25)",
      "  --self-test               run the parser assertions and exit",
    ].join("\n"),
  );
}

export function selfTest(): void {
  const sample = [
    "[1700000000001] [info] [subagent-observe] session=s1 stage=2 hop=acp_parse | {\"stage\":2,\"hop\":\"acp_parse\",\"reason\":\"turn_complete\",\"total\":5,\"kinds\":{\"tool_call\":3,\"tool_call_update\":2}}",
    "[1700000000002] [info] [subagent-observe] session=s1 stage=3 hop=tee | {\"stage\":3,\"hop\":\"tee\",\"enqueued\":7,\"written\":7,\"dropped\":0,\"fileLines\":7}",
    "[1700000000003] [info] [subagent-observe] session=s1 stage=5 hop=replay | {\"stage\":5,\"hop\":\"replay\",\"outcome\":\"applied\",\"totalTee\":7,\"afterCursor\":7,\"cursorSkips\":0,\"uncovered\":7,\"uncoveredFiltered\":0,\"applied\":6,\"dedupeSkips\":1,\"userEchoCards\":1,\"baselineAborts\":0,\"appliedByRole\":{\"tool\":3,\"assistant\":2,\"thought\":1}}",
    "[1700000000004] [info] [chief] session=s1 noise | not-json",
  ].join("\r\n");
  const lines = parseLogLines(sample);
  if (lines.length !== 4) throw new Error(`parseLogLines expected 4, got ${lines.length}`);
  const stages = collectStages(lines).get("s1");
  if (!stages || stages.stage2?.total !== 5) throw new Error("stage 2 fold failed");
  if (stages.stage2.kinds.tool_call !== 3) throw new Error("stage 2 kind fold failed");
  if (stages.stage3?.written !== 7) throw new Error("stage 3 fold failed");
  if (stage5Role(stages.stage5, "tool") !== 3) throw new Error("stage 5 role fold failed");
  if (stages.stage5.outcomes.applied !== 1) throw new Error("stage 5 outcome fold failed");

  const teeLines = [
    JSON.stringify({
      seq: 1,
      method: "session/update",
      data: { params: { update: { sessionUpdate: "tool_call" } } },
    }),
    JSON.stringify({
      seq: 2,
      method: "session/update",
      data: { params: { update: { sessionUpdate: "tool_call_update" } } },
    }),
    JSON.stringify({ seq: 3, method: "turn/complete", data: {} }),
  ];
  const teeScan = summarizeKindCounts(teeLines, foldTeeLine);
  if (teeScan.totals.tool_call !== 1) throw new Error("tee fold failed");
  if (teeScan.totals.tool_call_update !== 1) throw new Error("tee update fold failed");
  if (teeScan.totals["event:turn/complete"] !== 1) throw new Error("tee event fold failed");

  const transcriptLines = [
    JSON.stringify({ type: "tool_call" }),
    JSON.stringify({ type: "tool_call" }),
    JSON.stringify({ type: "assistant_message" }),
    "",
  ];
  const transcriptScan = summarizeKindCounts(transcriptLines, foldTranscriptLine);
  if (transcriptScan.totals.tool_call !== 2) throw new Error("transcript fold failed");
  if (transcriptScan.lines !== 3) throw new Error("transcript line count failed");

  const baselines = parseBaselineFile(
    JSON.stringify({
      s1: { total: 492, toolCalls: 127 },
      s2: 84,
    }),
  );
  if (baselines.s1?.toolCalls !== 127 || baselines.s2?.total !== 84) {
    throw new Error("baseline file parse failed");
  }

  const report = buildSessionReport(
    "s1",
    stages,
    { path: "tee", totals: teeScan.totals, lines: 3 },
    { path: "transcript", totals: transcriptScan.totals, lines: 3 },
    { total: 492, toolCalls: 3 },
  );
  if (report.notes.some((note) => note.startsWith("BREAK"))) {
    throw new Error("verdict flagged a break on a healthy sample");
  }
  const text = renderSession(report);
  if (!text.includes("tool_call by hop")) throw new Error("render missing hop line");

  // A collapse must be flagged: 127 tool calls at tee, transcript empty,
  // replay never observed.
  const broken = buildSessionReport(
    "s2",
    { stage5: { attempts: 0, latest: null, outcomes: {}, lastTs: 0 } },
    { path: "tee", totals: { tool_call: 127 }, lines: 127 },
    { path: null, totals: {}, lines: 0 },
    { toolCalls: 127 },
  );
  if (!broken.notes.some((note) => note.includes("transcript is empty"))) {
    throw new Error("verdict missed an empty-transcript break");
  }
  console.log("report-subagent-pipeline self-test: OK");
}

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2));
  if (options.selfTest) {
    selfTest();
    return;
  }

  if (options.baselineFile) {
    try {
      Object.assign(options.baselines, parseBaselineFile(readFileSync(options.baselineFile, "utf8")));
    } catch {
      console.error(`could not read baseline file: ${options.baselineFile}`);
    }
  }

  const logFiles = options.logs.length
    ? options.logs
    : DEFAULT_LOG_FILES.map((name) => path.join(options.home, ".marionette", "logs", name));
  const allLines: ParsedLogLine[] = [];
  for (const file of logFiles) {
    const lines = await readLines(file);
    if (lines) allLines.push(...parseLogLines(lines.join("\n")));
  }
  const stages = collectStages(allLines);
  const projects = readProjects(options.home);

  const wanted = options.sessions.length
    ? options.sessions
    : [...stages.entries()]
        .filter(([, entry]) => entry.stage2 || entry.stage3 || entry.stage5.attempts > 0)
        .sort((a, b) => {
          const aTs = Math.max(a[1].stage2?.ts ?? 0, a[1].stage3?.ts ?? 0, a[1].stage5.lastTs);
          const bTs = Math.max(b[1].stage2?.ts ?? 0, b[1].stage3?.ts ?? 0, b[1].stage5.lastTs);
          return bTs - aTs;
        })
        .slice(0, options.maxSessions)
        .map(([sessionId]) => sessionId);

  if (wanted.length === 0) {
    console.log("no subagent-observe lines found");
    console.log(`logs: ${logFiles.join(", ")}`);
    return;
  }

  const reports: SessionReport[] = [];
  for (const sessionId of wanted) {
    const entry = stages.get(sessionId) ?? {
      stage5: { attempts: 0, latest: null, outcomes: {}, lastTs: 0 },
    };
    const teePath = findTeePath(sessionId, options.home);
    const teeLines = await readLines(teePath);
    const teeScan = teeLines
      ? { ...summarizeKindCounts(teeLines, foldTeeLine), path: teePath }
      : { path: existsSync(teePath) ? teePath : null, totals: {}, lines: 0, scanError: "not readable" };

    const transcriptPath = findTranscriptPath(sessionId, options.home, projects);
    const transcriptLines = transcriptPath ? await readLines(transcriptPath) : null;
    const transcriptScan = transcriptPath
      ? { ...summarizeKindCounts(transcriptLines ?? [], foldTranscriptLine), path: transcriptPath }
      : { path: null, totals: {}, lines: 0 };

    reports.push(
      buildSessionReport(sessionId, entry, teeScan, transcriptScan, options.baselines[sessionId]),
    );
  }

  if (options.json) {
    console.log(JSON.stringify(reports, null, 2));
    return;
  }

  console.log(
    "P0-1 subagent pipeline: same-session, five-hop comparison (tool_call is the comparable unit)",
  );
  for (const report of reports) {
    console.log("");
    console.log(renderSession(report));
  }
  console.log("");
  console.log("note: total counts are not expected to match across hops; agent_message/thought");
  console.log("chunks are coalesced on purpose and tool_call_update folds into a tool_call card.");
  console.log("Watch tool_call (distinct calls) to find the hop that loses events.");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
