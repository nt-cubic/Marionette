import type { SessionEvent } from "./types";
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
    if (values.length === 1) return values[0].trim();
  } catch {
    // Not JSON — plain string input.
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
