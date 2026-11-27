/**
 * Capture a real delegated-worker event stream and write a sanitized replay
 * fixture (P0-8).
 *
 * Input: a chief/worker tee file (`~/.marionette/chief/sessions/<id>.events.jsonl`)
 * or any JSONL with the same `{ sessionId, kind, method, data, seq, ts }` shape.
 *
 * Output:
 *   scripts/fixtures/subagent-worker.events.jsonl
 *   scripts/fixtures/subagent-worker.manifest.json
 *
 * Design rules:
 * - derive every protocol frame from the real file (no hand-written stream);
 *   only ONE synthetic user_message_chunk seed is added because tee files do
 *   not record user prompts and the replay's user-echo branch needs a seed.
 * - keep event kinds, ids' equality relationships, statuses, ordering,
 *   delta/snapshot semantics and tool-call linkage.
 * - replace prompts, message bodies, paths, repo names, URLs, tokens and
 *   command output with deterministic markers.
 * - byte-stable output for the same source: ids are assigned in first-seen
 *   order, text is content-addressed, timestamps are anchored to a fixed epoch,
 *   and seq is reassigned densely from 1.
 *
 * Usage:
 *   npx tsx scripts/capture-subagent-fixture.ts
 *   npx tsx scripts/capture-subagent-fixture.ts --source <events.jsonl>
 *   npx tsx scripts/capture-subagent-fixture.ts --check
 *
 * `--check` regenerates in memory and fails if the committed fixture differs.
 * It never writes. Do not point it at a file you cannot read.
 */
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  parseSubagentFixture,
  summarizeSubagentFixture,
  validateSubagentFixture,
  type SubagentFixtureEvent,
} from "./fixtures/subagentFixture.ts";

const SANITIZER_VERSION = 1;
const FIXTURE_SESSION_ID = "session-fixture-worker";
const FIXTURE_INNER_SESSION_ID = "ses_fixture_worker";
/** Fixed anchor so the fixture does not carry the real run date. */
const TS_ANCHOR = Date.UTC(2024, 0, 1, 0, 0, 0, 0);
const KEEP_UPDATE_KINDS = new Set([
  "user_message_chunk",
  "agent_thought_chunk",
  "agent_message_chunk",
  "tool_call",
  "tool_call_update",
]);
const KEEP_METHODS = new Set(["turn/complete"]);

const DEFAULT_SOURCE =
  process.env.MARIONETTE_SUBAGENT_FIXTURE_SOURCE ??
  path.join(
    homedir(),
    ".marionette",
    "chief",
    "sessions",
    "session-1795778270746.events.jsonl",
  );
const HERE = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_OUT_DIR = path.join(HERE, "fixtures");
const FIXTURE_NAME = "subagent-worker.events.jsonl";
const MANIFEST_NAME = "subagent-worker.manifest.json";

type RawEvent = {
  record: Record<string, unknown>;
  line: number;
};

type SourceStats = {
  bytes: number;
  lines: number;
  kinds: Record<string, number>;
  toolCallStarts: number;
  distinctToolCallIds: number;
  firstTs: number | null;
  lastTs: number | null;
};

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function sourceUpdate(record: Record<string, unknown>): Record<string, unknown> | null {
  const data = asRecord(record.data);
  const params = data ? asRecord(data.params) : null;
  return params ? asRecord(params.update) : null;
}

function sourceEventKind(record: Record<string, unknown>): string {
  const update = sourceUpdate(record);
  const kind = update?.sessionUpdate;
  if (typeof kind === "string" && kind) return kind;
  const method = record.method;
  return typeof method === "string" && method ? `event:${method}` : "unknown";
}

function sourceToolCallId(record: Record<string, unknown>): string {
  const update = sourceUpdate(record);
  const id = update?.toolCallId ?? update?.tool_call_id;
  return typeof id === "string" ? id : "";
}

function readSource(sourcePath: string): { text: string; raw: RawEvent[]; stats: SourceStats } {
  if (!existsSync(sourcePath)) {
    throw new Error(
      `source tee file not found: ${sourcePath}\n` +
        "Pass --source <events.jsonl> or set MARIONETTE_SUBAGENT_FIXTURE_SOURCE.",
    );
  }
  const text = readFileSync(sourcePath, "utf8");
  const stats: SourceStats = {
    bytes: Buffer.byteLength(text, "utf8"),
    lines: 0,
    kinds: {},
    toolCallStarts: 0,
    distinctToolCallIds: 0,
    firstTs: null,
    lastTs: null,
  };
  const raw: RawEvent[] = [];
  const toolIds = new Set<string>();
  const lines = text.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    if (!line.trim()) continue;
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      throw new Error(`source line ${index + 1} is not valid JSON`);
    }
    const record = asRecord(parsed);
    if (!record) throw new Error(`source line ${index + 1} is not a JSON object`);
    raw.push({ record, line: index + 1 });
    stats.lines += 1;
    const kind = sourceEventKind(record);
    stats.kinds[kind] = (stats.kinds[kind] ?? 0) + 1;
    if (kind === "tool_call") {
      stats.toolCallStarts += 1;
      const id = sourceToolCallId(record);
      if (id) toolIds.add(id);
    }
    const ts = record.ts;
    if (typeof ts === "number" && Number.isFinite(ts)) {
      stats.firstTs = stats.firstTs === null ? ts : Math.min(stats.firstTs, ts);
      stats.lastTs = stats.lastTs === null ? ts : Math.max(stats.lastTs, ts);
    }
  }
  stats.distinctToolCallIds = toolIds.size;
  return { text, raw, stats };
}

const SEMANTIC_KEYS = new Set([
  "sessionUpdate",
  "type",
  "kind",
  "status",
  "stopReason",
  "role",
  "severity",
  "jsonrpc",
  "method",
]);
const TITLE_KEYS = new Set(["title"]);
const PATH_KEYS = new Set([
  "path",
  "filePath",
  "filepath",
  "file_path",
  "cwd",
  "root",
  "rootPath",
  "dir",
  "directory",
  "filename",
  "fileName",
]);
const TOOL_NAME_KEYS = new Set(["name", "toolName", "tool_name"]);
const SAFE_TOOL_NAMES = new Set([
  "bash",
  "read",
  "write",
  "edit",
  "grep",
  "glob",
  "task",
  "agent",
  "spawn_subagent",
  "webfetch",
  "websearch",
  "patch",
  "shell",
  "command",
  "tool",
]);
/**
 * Semantic string values are protocol vocabulary, not free text. A value
 * outside this set is replaced with a marker, so a model name or other
 * metadata that happens to sit under `kind`/`type`/`name` cannot ride along.
 */
const SAFE_SEMANTIC_VALUES = new Set([
  // envelopes / methods
  "session/update",
  "turn/complete",
  "notification",
  "response",
  "system",
  "process/stopped",
  "rpc/response",
  // ACP update kinds we keep (plus common non-kept ones)
  "user_message_chunk",
  "agent_thought_chunk",
  "agent_message_chunk",
  "tool_call",
  "tool_call_update",
  "available_commands_update",
  "config_option_update",
  "usage_update",
  // content block types
  "text",
  "content",
  "diff",
  "file",
  "select",
  "image",
  "audio",
  "resource",
  "blob",
  "json",
  "code",
  "url",
  "path",
  "tool_use",
  "tool_result",
  // statuses
  "pending",
  "in_progress",
  "completed",
  "failed",
  "cancelled",
  "error",
  "running",
  "starting",
  "waiting",
  "waiting_user",
  "timeout",
  "denied",
  "skipped",
  // tool kinds
  "execute",
  "read",
  "edit",
  "search",
  "other",
  "tool",
  "fetch",
  "browser",
  // tool title labels (see labelForKind)
  "Execute",
  "Read",
  "Edit",
  "Search",
  "Tool",
  // stop reasons
  "end_turn",
  "max_tokens",
  "refusal",
  "max_turn_requests",
  "pause_turn",
  // roles / severities
  "assistant",
  "thought",
  "user",
  "info",
  "warning",
  // jsonrpc
  "2.0",
]);
const SEMANTIC_BLOCKLIST =
  /marionette|deepseek|openai|anthropic|opencode|zhipu|grok|gemini|claude|codex|github|http|bearer|api[_-]?key|gpt|llama|mistral|qwen|kimi|glm|minimax|moonshot|bytedance|nvidia|cohere|perplexity|xai|x-ai/i;

function isSafeSemanticValue(value: string): boolean {
  return SAFE_SEMANTIC_VALUES.has(value) && !SEMANTIC_BLOCKLIST.test(value);
}

function labelForKind(kind: string): string {
  switch (kind) {
    case "execute":
      return "Execute";
    case "read":
      return "Read";
    case "edit":
      return "Edit";
    case "search":
      return "Search";
    default:
      return "Tool";
  }
}

class Sanitizer {
  private toolIdMap = new Map<string, string>();
  private messageIdMap = new Map<string, string>();
  private textMap = new Map<string, string>();
  private pathMap = new Map<string, string>();
  private urlMap = new Map<string, string>();
  /** Last kind seen per original tool id, so a bare status update keeps its label. */
  private toolKind = new Map<string, string>();

  get counts(): {
    toolIds: number;
    messageIds: number;
    texts: number;
    paths: number;
    urls: number;
  } {
    return {
      toolIds: this.toolIdMap.size,
      messageIds: this.messageIdMap.size,
      texts: this.textMap.size,
      paths: this.pathMap.size,
      urls: this.urlMap.size,
    };
  }

  private toolId(value: string): string {
    const existing = this.toolIdMap.get(value);
    if (existing) return existing;
    const mapped = `call_fixture_${String(this.toolIdMap.size + 1).padStart(4, "0")}`;
    this.toolIdMap.set(value, mapped);
    return mapped;
  }

  private messageId(value: string): string {
    const existing = this.messageIdMap.get(value);
    if (existing) return existing;
    const mapped = `msg_fixture_${String(this.messageIdMap.size + 1).padStart(4, "0")}`;
    this.messageIdMap.set(value, mapped);
    return mapped;
  }

  private text(value: string): string {
    if (value === "") return "";
    // Whitespace-only stream tokens matter for merge semantics; keep the class
    // but canonicalize the payload so no original spacing survives.
    if (/^\s+$/.test(value)) return " ";
    const existing = this.textMap.get(value);
    if (existing) return existing;
    const mapped = `[redacted #${this.textMap.size + 1}]`;
    this.textMap.set(value, mapped);
    return mapped;
  }

  private path(value: string): string {
    const existing = this.pathMap.get(value);
    if (existing) return existing;
    const mapped = `/redacted/path-${String(this.pathMap.size + 1).padStart(4, "0")}`;
    this.pathMap.set(value, mapped);
    return mapped;
  }

  private url(value: string): string {
    const existing = this.urlMap.get(value);
    if (existing) return existing;
    const mapped = `redacted-url-${this.urlMap.size + 1}`;
    this.urlMap.set(value, mapped);
    return mapped;
  }

  private string(value: string, key: string): string {
    if (key === "sessionId" || key === "session_id") {
      return key === "sessionId" ? FIXTURE_SESSION_ID : FIXTURE_INNER_SESSION_ID;
    }
    if (key === "toolCallId" || key === "tool_call_id") return this.toolId(value);
    if (key === "messageId" || key === "message_id") return this.messageId(value);
    if (TITLE_KEYS.has(key)) return "Tool"; // replaced with a kind label below
    if (PATH_KEYS.has(key)) return this.path(value);
    if (/^(?:url|uri|href|endpoint|link)$/i.test(key)) return this.url(value);
    if (TOOL_NAME_KEYS.has(key)) {
      return SAFE_TOOL_NAMES.has(value) ? value : "tool";
    }
    if (SEMANTIC_KEYS.has(key) && isSafeSemanticValue(value)) return value;
    return this.text(value);
  }

  value(value: unknown, key: string): unknown {
    if (typeof value === "string") return this.string(value, key);
    if (typeof value === "number" || typeof value === "boolean" || value === null) {
      return value;
    }
    if (Array.isArray(value)) return value.map((item) => this.value(item, key));
    if (value && typeof value === "object") {
      const out: Record<string, unknown> = {};
      for (const [childKey, childValue] of Object.entries(value)) {
        out[childKey] = this.value(childValue, childKey);
      }
      return out;
    }
    return value;
  }

  update(update: Record<string, unknown>): Record<string, unknown> {
    const originalId =
      typeof update.toolCallId === "string"
        ? update.toolCallId
        : typeof update.tool_call_id === "string"
          ? update.tool_call_id
          : "";
    const ownKind = typeof update.kind === "string" ? update.kind : "";
    if (originalId && ownKind) this.toolKind.set(originalId, ownKind);
    const kind = ownKind || (originalId ? (this.toolKind.get(originalId) ?? "") : "");
    const out = this.value(update, "") as Record<string, unknown>;
    if (typeof update.title === "string") out.title = labelForKind(kind);
    return out;
  }

  data(record: Record<string, unknown>): unknown {
    const method = typeof record.method === "string" ? record.method : "";
    const data = asRecord(record.data);
    if (method === "session/update" && data) {
      const params = asRecord(data.params);
      const update = params ? asRecord(params.update) : null;
      const out: Record<string, unknown> = {};
      for (const [key, value] of Object.entries(data)) {
        if (key === "params") continue;
        out[key] = this.value(value, key);
      }
      if (params) {
        const nextParams: Record<string, unknown> = {};
        for (const [key, value] of Object.entries(params)) {
          if (key === "update" || key === "sessionId") continue;
          nextParams[key] = this.value(value, key);
        }
        nextParams.sessionId = FIXTURE_INNER_SESSION_ID;
        if (update) nextParams.update = this.update(update);
        out.params = nextParams;
      }
      return out;
    }
    return this.value(record.data, "data");
  }
}

type BuildResult = {
  events: SubagentFixtureEvent[];
  fixtureText: string;
  sanitizer: Sanitizer;
  droppedKinds: Record<string, number>;
  keptKinds: Record<string, number>;
};

function buildFixture(raw: RawEvent[], stats: SourceStats): BuildResult {
  const kept: RawEvent[] = [];
  const droppedKinds: Record<string, number> = {};
  const keptKinds: Record<string, number> = {};
  for (const entry of raw) {
    const kind = sourceEventKind(entry.record);
    const method = typeof entry.record.method === "string" ? entry.record.method : "";
    const keep =
      (method === "session/update" && KEEP_UPDATE_KINDS.has(kind)) || KEEP_METHODS.has(method);
    if (keep) {
      kept.push(entry);
      keptKinds[kind] = (keptKinds[kind] ?? 0) + 1;
    } else {
      droppedKinds[kind] = (droppedKinds[kind] ?? 0) + 1;
    }
  }

  const minTs = stats.firstTs ?? 0;
  const sanitizer = new Sanitizer();
  const events: SubagentFixtureEvent[] = [];

  // ONE synthetic seed: the tee records agent notifications only, so the
  // replay's user-echo branch never has a real user frame. Content is a fixed
  // placeholder, not user input.
  events.push({
    sessionId: FIXTURE_SESSION_ID,
    kind: "notification",
    method: "session/update",
    data: {
      jsonrpc: "2.0",
      method: "session/update",
      params: {
        sessionId: FIXTURE_INNER_SESSION_ID,
        update: {
          sessionUpdate: "user_message_chunk",
          messageId: "msg_fixture_user_0001",
          content: { type: "text", text: "[redacted user prompt]" },
        },
      },
    },
    seq: 1,
    ts: TS_ANCHOR - 1,
  });

  let seq = 2;
  for (const entry of kept) {
    const record = entry.record;
    const originalTs =
      typeof record.ts === "number" && Number.isFinite(record.ts) ? record.ts : minTs;
    const method = typeof record.method === "string" ? record.method : "";
    events.push({
      sessionId: FIXTURE_SESSION_ID,
      kind: typeof record.kind === "string" ? record.kind : "notification",
      method,
      data: sanitizer.data(record),
      seq,
      ts: TS_ANCHOR + Math.max(0, originalTs - minTs),
    });
    seq += 1;
  }

  const fixtureText = `${events.map((event) => JSON.stringify(event)).join("\n")}\n`;
  return { events, fixtureText, sanitizer, droppedKinds, keptKinds };
}

const FIXED_FORBIDDEN = [
  "C:\\Users",
  "D:\\Project",
  "Marionette",
  "github.com",
  "https://",
  "http://",
  "Bearer ",
  "Authorization",
  "api_key",
  "openai",
  "deepseek",
  "anthropic",
  "opencode",
  "zhipu",
  "sk-",
];

/** Collect originals that must not survive into the fixture. */
function sourceForbiddenTokens(text: string, raw: RawEvent[]): string[] {
  const tokens = new Set<string>(FIXED_FORBIDDEN);
  const ids = new Set<string>();
  for (const entry of raw) {
    const record = entry.record;
    const sessionId = record.sessionId;
    if (typeof sessionId === "string" && sessionId) ids.add(sessionId);
    const update = sourceUpdate(record);
    if (update) {
      for (const key of ["toolCallId", "tool_call_id", "messageId", "message_id"]) {
        const value = update[key];
        if (typeof value === "string" && value.length >= 6) ids.add(value);
      }
    }
  }
  for (const id of ids) tokens.add(id);
  for (const match of text.match(/[A-Za-z]:\\[^\s"'<>|]+/g) ?? []) {
    if (match.length >= 8) tokens.add(match);
  }
  for (const match of text.match(/https?:\/\/[^\s"'<>)]+/g) ?? []) {
    tokens.add(match);
  }
  return [...tokens].filter((token) => token.length >= 6);
}

function assertNoForbiddenStrings(fixtureText: string, tokens: string[]): void {
  for (const token of tokens) {
    if (fixtureText.includes(token)) {
      const digest = sha256(token).slice(0, 12);
      throw new Error(
        `sanitizer left a forbidden original string (sha256:${digest}, length ${token.length})`,
      );
    }
  }
}

const MARKER_PATTERNS = [
  /^\[redacted #\d+\]$/,
  /^\/redacted\/path-\d{4}$/,
  /^redacted-url-\d+$/,
  /^call_fixture_\d{4}$/,
  /^msg_fixture_\d{4}$/,
];
const FIXED_SAFE_VALUES = new Set([
  FIXTURE_SESSION_ID,
  FIXTURE_INNER_SESSION_ID,
  "msg_fixture_user_0001",
  "[redacted user prompt]",
]);

function assertSafeStringValues(events: readonly SubagentFixtureEvent[]): void {
  const errors: string[] = [];
  const visit = (value: unknown, trail: string): void => {
    if (typeof value === "string") {
      if (!value || /^\s+$/.test(value)) return;
      if (FIXED_SAFE_VALUES.has(value)) return;
      if (MARKER_PATTERNS.some((pattern) => pattern.test(value))) return;
      if (SAFE_SEMANTIC_VALUES.has(value) || SAFE_TOOL_NAMES.has(value)) return;
      errors.push(`${trail}: length ${value.length}`);
      return;
    }
    if (Array.isArray(value)) {
      value.forEach((item, index) => visit(item, `${trail}[${index}]`));
      return;
    }
    if (value && typeof value === "object") {
      for (const [key, child] of Object.entries(value)) {
        visit(child, trail ? `${trail}.${key}` : key);
      }
    }
  };
  for (let index = 0; index < events.length; index += 1) {
    visit(events[index], `event[${index}]`);
  }
  if (errors.length > 0) {
    throw new Error(
      `sanitized fixture still has non-marker string values (${errors.length}):\n` +
        errors.slice(0, 10).join("\n"),
    );
  }
}

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

type CliOptions = {
  source: string;
  outDir: string;
  capturedAt: string | null;
  check: boolean;
};

function parseArgs(argv: string[]): CliOptions {
  const options: CliOptions = {
    source: DEFAULT_SOURCE,
    outDir: DEFAULT_OUT_DIR,
    capturedAt: null,
    check: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--source") options.source = argv[++index] ?? options.source;
    else if (arg === "--out") options.outDir = argv[++index] ?? options.outDir;
    else if (arg === "--captured-at") options.capturedAt = argv[++index] ?? null;
    else if (arg === "--check") options.check = true;
    else if (arg === "--help" || arg === "-h") {
      printHelp();
      process.exit(0);
    }
  }
  return options;
}

function printHelp(): void {
  console.log(
    [
      "capture-subagent-fixture.ts",
      "",
      "  --source <events.jsonl>  real worker tee to sanitize (default: env or the",
      "                           known local capture)",
      "  --out <dir>              output directory (default: scripts/fixtures)",
      "  --captured-at <ISO>      override the provenance date (default: source date)",
      "  --check                  regenerate in memory and fail if committed files differ",
    ].join("\n"),
  );
}

function capture(options: CliOptions): {
  fixtureText: string;
  manifestText: string;
  manifest: Record<string, unknown>;
} {
  const { text, raw, stats } = readSource(options.source);
  const { events, fixtureText, sanitizer, droppedKinds, keptKinds } = buildFixture(raw, stats);

  const validation = validateSubagentFixture(events);
  if (!validation.ok) {
    throw new Error(`fixture validation failed:\n- ${validation.errors.join("\n- ")}`);
  }
  assertNoForbiddenStrings(fixtureText, sourceForbiddenTokens(text, raw));
  assertSafeStringValues(events);

  const fixtureSummary = summarizeSubagentFixture(events);
  const counts = sanitizer.counts;
  const capturedAt =
    options.capturedAt ??
    new Date(stats.lastTs ?? Date.UTC(2024, 0, 1)).toISOString().slice(0, 10);

  const manifest = {
    schemaVersion: 1,
    fixture: FIXTURE_NAME,
    fixtureSha256: sha256(fixtureText),
    capturedAt,
    capturedFrom: {
      kind: "chief worker tee (ACP session/update JSONL)",
      sourceRef: "local worker session tee; absolute path and session id withheld",
      sourceBytes: stats.bytes,
      sourceLines: stats.lines,
      sourceKinds: stats.kinds,
      sourceToolCallStarts: stats.toolCallStarts,
      sourceDistinctToolCallIds: stats.distinctToolCallIds,
    },
    fixtureCounts: {
      lines: fixtureSummary.lines,
      kinds: fixtureSummary.kinds,
      toolCallStarts: fixtureSummary.toolCallStarts,
      toolCallUpdates: fixtureSummary.toolCallUpdates,
      distinctToolCallIds: fixtureSummary.distinctToolCallIds,
      userChunks: fixtureSummary.userChunks,
      thoughtChunks: fixtureSummary.thoughtChunks,
      assistantChunks: fixtureSummary.assistantChunks,
      turnCompletions: fixtureSummary.turnCompletions,
      firstSeq: fixtureSummary.firstSeq,
      lastSeq: fixtureSummary.lastSeq,
    },
    transforms: {
      keptKinds: [...KEEP_UPDATE_KINDS],
      keptMethods: [...KEEP_METHODS],
      droppedKinds,
      keptOriginalKinds: keptKinds,
      seq: "reassigned densely 1..N in kept order; synthetic user seed is seq 1",
      ts: `anchor ${TS_ANCHOR} + (original ts - first source ts); user seed = anchor - 1`,
      sessionId: `${FIXTURE_SESSION_ID} / ${FIXTURE_INNER_SESSION_ID}`,
      ids: `${counts.toolIds} tool ids and ${counts.messageIds} message ids remapped in first-seen order; equality relationships preserved`,
      strings: `${counts.texts} distinct non-whitespace strings replaced with [redacted #n]; whitespace-only -> single space`,
      paths: `${counts.paths} distinct path-like values replaced with /redacted/path-N`,
      urls: `${counts.urls} distinct URL-like values replaced with redacted-url-N`,
      titles: "tool titles replaced with kind label (Execute/Read/Edit/Search/Tool)",
      contents: "content block shape preserved; text/delta/output bodies replaced",
    },
    synthetic: {
      frames: 1,
      kinds: ["user_message_chunk"],
      content: "[redacted user prompt]",
      reason:
        "tee files carry agent session/update notifications but not the user prompt; this single seed exercises the replay user-echo branch before agent activity",
    },
    sanitizer: {
      script: "scripts/capture-subagent-fixture.ts",
      version: SANITIZER_VERSION,
      removed: [
        "prompts and assistant/thought bodies",
        "file paths, repo names and usernames",
        "URLs",
        "tokens/keys/authorization values",
        "command input and output",
      ],
      limitations: [
        "structural JSON keys are preserved; a secret embedded in an unusual key name would not be scrubbed",
        "whitespace-only chunks are canonicalized to one space, so exact spacing is not preserved",
        "absolute run time is replaced by a fixed anchor; timestamps preserve relative gaps only",
        "only ACP session/update kinds and turn/complete are kept; rpc/config/usage/process noise is dropped",
        "text equality is preserved as equality classes (identical originals share a marker); individual characters are not",
        "the user seed is synthetic because tee files do not record user prompts",
      ],
    },
  };
  const manifestText = `${JSON.stringify(manifest, null, 2)}\n`;
  return { fixtureText, manifestText, manifest };
}

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2));
  const result = capture(options);
  const fixturePath = path.join(options.outDir, FIXTURE_NAME);
  const manifestPath = path.join(options.outDir, MANIFEST_NAME);

  if (options.check) {
    const problems: string[] = [];
    if (!existsSync(fixturePath)) problems.push(`missing ${fixturePath}`);
    else if (readFileSync(fixturePath, "utf8") !== result.fixtureText) {
      problems.push(`${fixturePath} differs from a fresh capture`);
    }
    if (!existsSync(manifestPath)) problems.push(`missing ${manifestPath}`);
    else if (readFileSync(manifestPath, "utf8") !== result.manifestText) {
      problems.push(`${manifestPath} differs from a fresh capture`);
    }
    if (problems.length > 0) {
      console.error("fixture check failed:");
      for (const problem of problems) console.error(`- ${problem}`);
      process.exitCode = 1;
      return;
    }
    console.log("subagent fixture check: OK");
    return;
  }

  writeFileSync(fixturePath, result.fixtureText, "utf8");
  writeFileSync(manifestPath, result.manifestText, "utf8");
  const counts = result.manifest.fixtureCounts as Record<string, unknown>;
  console.log(
    `wrote ${fixturePath} (${counts.lines} lines, ${counts.toolCallStarts} tool_call starts)`,
  );
  console.log(`wrote ${manifestPath}`);
  console.log(
    `sha256 ${String(result.manifest.fixtureSha256).slice(0, 16)}...  ` +
      `kinds ${JSON.stringify(counts.kinds)}`,
  );
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
