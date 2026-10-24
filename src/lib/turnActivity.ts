import type { SessionEvent } from "./types";
import { isSubagentTool } from "./activityHealth";
import { normalizeToolName } from "./toolCallNormalize";

/**
 * Per-tool-call classification + Claude-Code-style labels ("Read",
 * "Ran a command"). Pure data — no UI. Works for every ACP agent because tool
 * calls already arrive as normalized `tool_call` events with `toolName`/`path`.
 */

export type ToolActivityKind =
  | "read"
  | "edit"
  | "write"
  | "command"
  | "search"
  | "web"
  | "agent"
  | "other";

export type ToolCallEvent = Extract<SessionEvent, { type: "tool_call" }>;

/** Map a normalized tool name to a display bucket. */
function classifyName(name: string): ToolActivityKind {
  if (!name) return "other";
  if (name === "read" || name.startsWith("read_")) return "read";
  if (
    name === "edit" ||
    name === "write" ||
    name === "multi_edit" ||
    name === "apply_patch" ||
    name === "replace" ||
    name.startsWith("edit") ||
    name.startsWith("write") ||
    name.startsWith("patch") ||
    name.startsWith("change")
  ) {
    return name.startsWith("write") ? "write" : "edit";
  }
  if (
    name === "bash" ||
    name === "shell" ||
    name === "cmd" ||
    name === "powershell" ||
    name === "pwsh" ||
    name === "terminal" ||
    name.startsWith("exec") ||
    name.startsWith("run") ||
    name.startsWith("command")
  ) {
    return "command";
  }
  if (
    name === "grep" ||
    name === "glob" ||
    name.startsWith("search") ||
    name.startsWith("find") ||
    name.startsWith("list")
  ) {
    return "search";
  }
  if (
    name === "web_fetch" ||
    name === "web_search" ||
    name.startsWith("web") ||
    name.startsWith("browser") ||
    name.startsWith("http")
  ) {
    return "web";
  }
  if (
    name === "task" ||
    name === "agent" ||
    name === "subagent" ||
    /(task|agent|delegate|spawn)/.test(name)
  ) {
    return "agent";
  }
  return "other";
}

/** Stable bucket for a tool-call event (normalized name, title fallback). */
export function classifyToolCall(event: ToolCallEvent): ToolActivityKind {
  return classifyName(normalizeToolName(event.toolName || event.title));
}

/** Claude-Code-style verb for the tool row: Read / Edited / Ran a command… */
export function toolKindVerb(kind: ToolActivityKind): string {
  switch (kind) {
    case "read":
      return "Read";
    case "edit":
      return "Edited";
    case "write":
      return "Wrote";
    case "command":
      return "Ran a command";
    case "search":
      return "Searched";
    case "web":
      return "Fetched";
    case "agent":
      return "Task";
    default:
      return "Tool";
  }
}

/**
 * Human-readable command/query text from a tool card's `input`.
 * Claude/Codex wrap the real command in JSON (`{"command": "…"}`); Grok and
 * some adapters send it as a plain string. Input is already clipped upstream.
 */
export function extractCommandText(event: ToolCallEvent): string {
  const input = (event.input ?? "").trim();
  if (!input) return "";
  try {
    const parsed = JSON.parse(input) as Record<string, unknown>;
    for (const key of [
      "command",
      "cmd",
      "script",
      "prompt",
      "pattern",
      "query",
      "path",
      "filePath",
      "file_path",
      "url",
    ]) {
      const value = parsed[key];
      if (typeof value === "string" && value.trim()) return value.trim();
    }
    const values = Object.values(parsed).filter(
      (v): v is string => typeof v === "string" && v.trim().length > 0,
    );
    if (values.length === 1) return values[0]!.trim();
  } catch {
    // Not JSON, or JSON clipped mid-string upstream (`{"variant":"Bash","command":"powershell -NoProfi`).
    const clipped = /"(?:command|cmd|script|query|pattern|path|filePath|file_path|url)"\s*:\s*"((?:[^"\\]|\\.)*)/.exec(input);
    if (clipped?.[1]?.trim()) {
      return clipped[1].replace(/\\(["\\/])/g, "$1").replace(/\\[nrt]/g, " ").trim();
    }
  }
  return input.replace(/^\{|\}$/g, "").trim();
}

/**
 * Count changed lines from an edit/write `detail` that carries a unified diff
 * (Claude-Code-style "+27 −4"). Header lines (`---` / `+++`) are skipped.
 * Returns null when the detail holds no diff.
 */
export function extractDiffStats(
  detail: string | null | undefined,
): { add: number; del: number } | null {
  if (!detail) return null;
  let add = 0;
  let del = 0;
  for (const line of detail.split("\n")) {
    const trimmed = line.trim();
    if (trimmed.startsWith("+++") || trimmed.startsWith("---")) continue;
    if (trimmed.startsWith("+")) add += 1;
    else if (trimmed.startsWith("-")) del += 1;
  }
  if (add === 0 && del === 0) return null;
  return { add, del };
}

/** Basename of a tool path (`D:\\a\\b.ts` → `b.ts`). */
export function toolFileName(path: string | null | undefined): string {
  if (!path) return "";
  return path.split(/[\\/]/).filter(Boolean).pop() ?? "";
}

export function oneLine(s: string): string {
  return s.replace(/\s+/g, " ").trim();
}

export function clipTeaser(s: string, max = 72): string {
  const t = oneLine(s);
  if (!t) return "";
  return t.length > max ? `${t.slice(0, max - 1)}…` : t;
}

/**
 * One-line subject for a tool row: filename, command, query, or title.
 * Matches the Clean View card teaser (verb lives separately).
 */
export function toolMainText(event: ToolCallEvent): string {
  const kind = classifyToolCall(event);
  const fileName = toolFileName(event.path);
  const commandText = extractCommandText(event);
  const titleText = event.title ? oneLine(String(event.title)) : "";
  if (kind === "read" || kind === "edit" || kind === "write") {
    return fileName || commandText || titleText;
  }
  if (kind === "command" || kind === "search" || kind === "web") {
    return commandText || titleText || fileName;
  }
  return titleText || fileName || commandText;
}

export type ActivityKindBucket = ToolActivityKind | "thought";

export type ActivityDetailItem = {
  event: SessionEvent;
  index: number;
};

export type ActivityKindGroup = {
  bucket: ActivityKindBucket;
  items: ActivityDetailItem[];
};

export type ActivitySummary = {
  title: string;
  preview: string;
  /** Wall time of a finished run (`3m 33s`); empty while live or untimed. */
  elapsed?: string;
  /** Tool calls in the run that ended `failed` — shown in red, never only as color. */
  failed?: number;
  diff: { add: number; del: number } | null;
};

export function activityBucket(event: SessionEvent): ActivityKindBucket | null {
  if (event.type === "thought") return "thought";
  if (event.type === "tool_call") return classifyToolCall(event);
  return null;
}

export function isActivityEvent(event: SessionEvent): boolean {
  return activityBucket(event) != null;
}

export type ActivityClusterUnit =
  | { kind: "row"; key: string; event: SessionEvent; index: number }
  | { kind: "activity"; key: string; items: ActivityDetailItem[] }
;

export function isSubagentEvent(event: SessionEvent): boolean {
  return event.type === "tool_call" && isSubagentTool(event.toolName, event.title);
}

/**
 * Consecutive thought + tool_call runs become one activity fold.
 * A lone thought/tool stays a row (its own card already folds).
 */
export function clusterActivityRuns(
  units: { key: string; event: SessionEvent; index: number }[],
): ActivityClusterUnit[] {
  const clustered: ActivityClusterUnit[] = [];
  let i = 0;
  while (i < units.length) {
    const unit = units[i]!;
    if (!isActivityEvent(unit.event)) {
      clustered.push({ kind: "row", ...unit });
      i += 1;
      continue;
    }
    const start = i;
    i += 1;
    while (i < units.length && isActivityEvent(units[i]!.event)) i += 1;
    const slice = units.slice(start, i);
    if (slice.length === 1) {
      clustered.push({ kind: "row", ...slice[0]! });
    } else {
      clustered.push({
        kind: "activity",
        // Start index only — the run grows as tools stream in; a key that
        // included the last index remounted the fold on every new tool.
        key: `act-${slice[0]!.index}`,
        items: slice.map(({ event, index }) => ({ event, index })),
      });
    }
  }
  return clustered;
}

/** Consecutive same-kind runs (Read, Read, Edit → two groups). */
export function groupConsecutiveByKind(items: ActivityDetailItem[]): ActivityKindGroup[] {
  const groups: ActivityKindGroup[] = [];
  for (const item of items) {
    const bucket = activityBucket(item.event);
    if (!bucket) continue;
    const last = groups[groups.length - 1];
    if (last && last.bucket === bucket) last.items.push(item);
    else groups.push({ bucket, items: [item] });
  }
  return groups;
}

export function sumDiffStats(items: ActivityDetailItem[]): { add: number; del: number } | null {
  let add = 0;
  let del = 0;
  let any = false;
  for (const { event } of items) {
    if (event.type !== "tool_call") continue;
    const stats = extractDiffStats(event.detail);
    if (!stats) continue;
    any = true;
    add += stats.add;
    del += stats.del;
  }
  return any ? { add, del } : null;
}

function kindCountPhrase(bucket: ActivityKindBucket, count: number): string {
  switch (bucket) {
    case "thought":
      return count > 1 ? `Thought · ${count}` : "Thought";
    case "read":
      return `Read ${count} files`;
    case "edit":
      return `Edited ${count} files`;
    case "write":
      return `Wrote ${count} files`;
    case "command":
      return `Ran ${count} commands`;
    case "search":
      return `Searched ${count} times`;
    case "web":
      return `Fetched ${count} pages`;
    case "agent":
      return count === 1 ? "Task" : `${count} tasks`;
    default:
      return count === 1 ? "Tool" : `${count} tools`;
  }
}

/** Inline headline for a kind run: "Read foo.ts" / "Ran 3 commands". */
export function kindGroupTitle(group: ActivityKindGroup): string {
  const { bucket, items } = group;
  if (items.length === 1) {
    const event = items[0]!.event;
    if (event.type === "thought") return "Thought";
    if (event.type === "tool_call") {
      const verb = toolKindVerb(bucket === "thought" ? "other" : bucket);
      const main = toolMainText(event);
      return main ? `${verb} ${main}` : verb;
    }
  }
  return kindCountPhrase(bucket, items.length);
}

export function summarizeKindGroup(group: ActivityKindGroup): ActivitySummary {
  const title = kindGroupTitle(group);
  let preview = "";
  if (group.items.length === 1 && group.items[0]!.event.type === "tool_call") {
    // Title already has verb+main; keep preview empty so the row stays one phrase.
    preview = "";
  } else if (group.bucket === "command" && group.items.length > 1) {
    const last = group.items[group.items.length - 1]!.event;
    if (last.type === "tool_call") preview = toolMainText(last);
  }
  return {
    title,
    preview,
    failed: countFailed(group.items),
    diff: sumDiffStats(group.items),
  };
}

/** Plural of `tallyWord` for counts above one (`3 commands`, `2 searches`). */
function tallyPlural(bucket: ActivityKindBucket): string {
  const word = tallyWord(bucket);
  if (word === "other") return word;
  return /(ch|sh|s|x)$/.test(word) ? `${word}es` : `${word}s`;
}

/** Short type word for the closed activity tally (`1 thought, 2 reads`). */
export function tallyWord(bucket: ActivityKindBucket): string {
  switch (bucket) {
    case "thought":
      return "thought";
    case "read":
      return "read";
    case "edit":
      return "edit";
    case "write":
      return "write";
    case "command":
      return "command";
    case "search":
      return "search";
    case "web":
      return "fetch";
    case "agent":
      return "task";
    default:
      return "other";
  }
}

/**
 * Closed-fold tally: how many times each kind ran, in first-seen order.
 * `1 think, 2 read, 3 edit`
 */
export function formatActivityTally(items: ActivityDetailItem[]): string {
  const order: ActivityKindBucket[] = [];
  const counts = new Map<ActivityKindBucket, number>();
  for (const item of items) {
    const bucket = activityBucket(item.event);
    if (!bucket) continue;
    if (!counts.has(bucket)) order.push(bucket);
    counts.set(bucket, (counts.get(bucket) ?? 0) + 1);
  }
  return order
    .map((bucket) => {
      const n = counts.get(bucket) ?? 0;
      return `${n} ${n === 1 ? tallyWord(bucket) : tallyPlural(bucket)}`;
    })
    .join(", ");
}

/**
 * Headline for a consecutive thought/tool run.
 * Live: current step so the closed row tracks the work.
 * Done: count + type tally (`1 think, 2 read, 3 edit`).
 */
export function summarizeActivity(
  items: ActivityDetailItem[],
  opts?: { liveEvent?: SessionEvent | null },
): ActivitySummary {
  const live = opts?.liveEvent;
  if (live?.type === "thought") {
    return { title: "Thinking…", preview: "", diff: sumDiffStats(items) };
  }
  if (live?.type === "tool_call") {
    const verb = toolKindVerb(classifyToolCall(live));
    const main = toolMainText(live);
    return {
      title: main ? `${verb} ${main}` : verb,
      preview: "",
      diff: sumDiffStats(items),
    };
  }
  const tally = formatActivityTally(items);
  const span = activityTimeline(items);
  return {
    title: tally || "Activity",
    preview: "",
    elapsed: span && span.durationMs >= 1000 ? formatSpanDuration(span.durationMs) : "",
    failed: countFailed(items),
    diff: sumDiffStats(items),
  };
}

export type ActivitySpan = {
  bucket: ActivityKindBucket;
  /** ms from the run's first event. */
  start: number;
  end: number;
  failed: boolean;
  /** Still open (no terminal status yet). */
  open: boolean;
  event: SessionEvent;
};

export type ActivityTimelineData = {
  durationMs: number;
  /** Buckets in first-seen order — the same order as the closed tally. */
  lanes: ActivityKindBucket[];
  spans: ActivitySpan[];
};

function eventMs(value: string | undefined): number | null {
  if (!value) return null;
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : null;
}

/**
 * Time layout of one thought/tool run for the fold's timeline strip.
 * A step ends at its own terminal stamp (`completedAt` / `endedAt`), else
 * where the next step starts. Null when the run carries no usable times.
 */
export function activityTimeline(
  items: ActivityDetailItem[],
  nowMs?: number,
): ActivityTimelineData | null {
  const starts = items.map((item) => eventMs(item.event.createdAt));
  const first = starts.find((ms): ms is number => ms != null);
  if (first == null) return null;
  // Next known start per step, filled back to front so the run stays O(n).
  const nextStart: (number | null)[] = new Array(items.length).fill(null);
  for (let i = items.length - 2; i >= 0; i -= 1) nextStart[i] = starts[i + 1]! ?? nextStart[i + 1];
  const lanes: ActivityKindBucket[] = [];
  const spans: ActivitySpan[] = [];
  let last = first;
  items.forEach((item, i) => {
    const bucket = activityBucket(item.event);
    const start = starts[i];
    if (!bucket || start == null) return;
    if (!lanes.includes(bucket)) lanes.push(bucket);
    const ev = item.event;
    const status = ev.type === "tool_call" ? String(ev.status ?? "") : "";
    const open = ev.type === "tool_call" && (status === "pending" || status === "in_progress");
    const own =
      ev.type === "tool_call"
        ? eventMs(ev.completedAt)
        : ev.type === "thought"
          ? eventMs(ev.endedAt)
          : null;
    const next = nextStart[i];
    let end = own ?? next ?? (open && nowMs != null ? nowMs : start);
    if (end < start) end = start;
    last = Math.max(last, end);
    spans.push({ bucket, start: start - first, end: end - first, failed: status === "failed", open, event: ev });
  });
  if (spans.length === 0) return null;
  return { durationMs: last - first, lanes, spans };
}

export function countFailed(items: ActivityDetailItem[]): number {
  let n = 0;
  for (const item of items) {
    if (item.event.type === "tool_call" && item.event.status === "failed") n += 1;
  }
  return n;
}

/** One step's time: `0.4s`, `3.2s` under ten seconds, then the run format. */
export function formatStepDuration(ms: number): string {
  if (ms < 10_000) return `${Math.max(0.1, Math.round(ms / 100) / 10)}s`;
  return formatSpanDuration(ms);
}

/** `48s` under a minute, `2m 10s` after — same unit style as `Thought · 17s`. */
export function formatSpanDuration(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return rest ? `${m}m ${rest}s` : `${m}m`;
}
