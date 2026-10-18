/**
 * Claude Code (and similar ACP agents) send ExitPlanMode as
 * `session/request_permission` with the plan stuffed into toolCall.rawInput.
 * Grok uses a dedicated `_x.ai/exit_plan_mode` method instead.
 *
 * This module pulls the markdown out of that permission payload so the UI can
 * render a full plan page instead of a 160px JSON dump.
 */

export type PermissionPlanOption = {
  optionId?: string;
  name?: string;
  kind?: string;
};

export type PermissionPlanInput = {
  title?: string | null;
  detail?: string | null;
  options?: PermissionPlanOption[] | null;
  params?: unknown;
  planMarkdown?: unknown;
};

const PLAN_KEYS = [
  "plan",
  "planMarkdown",
  "planContent",
  "plan_markdown",
  "plan_content",
] as const;

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function trimPlan(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

/** JSON.parse a string that is itself a JSON object/array. */
function parseJsonBlob(value: string): unknown {
  const trimmed = value.trim();
  if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    return null;
  }
}

function planFromRecord(record: Record<string, unknown>): string | null {
  for (const key of PLAN_KEYS) {
    const found = trimPlan(record[key]);
    if (found) return found;
  }
  return null;
}

/**
 * Walk the shapes Claude Code / Codex / ACP actually send:
 *   { plan: "..." }
 *   { rawInput: { plan: "..." } }   // wire name after Codex maps `input` → rawInput
 *   { input: { plan: "..." } }
 *   { toolCall: { title, rawInput: { plan } } }
 *   { params: { toolCall: { rawInput: { plan } } } }
 * plus the same with snake_case.
 */
function planFromUnknown(value: unknown, depth = 0): string | null {
  if (depth > 4 || value == null) return null;
  if (typeof value === "string") {
    const blob = parseJsonBlob(value);
    if (blob) {
      const nested = planFromUnknown(blob, depth + 1);
      if (nested) return nested;
    }
    return null;
  }
  const record = asRecord(value);
  if (!record) return null;

  const direct = planFromRecord(record);
  if (direct) return direct;

  for (const nested of [
    record.rawInput ?? record.raw_input,
    record.input,
    record.toolCall ?? record.tool_call,
    record.params,
  ]) {
    const found = planFromUnknown(nested, depth + 1);
    if (found) return found;
  }

  return null;
}

export function permissionOptionTone(
  kind: string,
  name: string,
): "allow" | "deny" | "neutral" {
  const k = `${kind} ${name}`.toLowerCase();
  if (k.includes("allow") || k.includes("accept") || k.includes("yes")) return "allow";
  if (k.includes("reject") || k.includes("deny") || k.includes("cancel") || k.includes("no")) {
    return "deny";
  }
  return "neutral";
}

export function isPlanApprovalTitle(title: string | null | undefined): boolean {
  if (!title) return false;
  return /approve\s*plan|exit[\s_-]*plan|plan\s*approval|implement\s+(this\s+)?plan|计划待批/i.test(
    title,
  );
}

function optionsSuggestPlan(options: PermissionPlanOption[] | null | undefined): boolean {
  if (!options?.length) return false;
  return options.some((opt) => {
    const blob = `${opt.kind ?? ""} ${opt.name ?? ""} ${opt.optionId ?? ""}`.toLowerCase();
    return (
      blob.includes("keep planning") ||
      blob.includes("keep_planning") ||
      blob.includes("exit_plan") ||
      blob.includes("exit-plan") ||
      blob.includes("implement_plan") ||
      blob.includes("revise_plan") ||
      blob.includes("implement this plan") ||
      /approve edits/.test(blob)
    );
  });
}

function looksLikeMarkdown(text: string | null | undefined): boolean {
  if (!text) return false;
  const trimmed = text.trim();
  if (!trimmed || trimmed.startsWith("{") || trimmed.startsWith("[")) return false;
  if (trimmed.startsWith("#")) return true;
  if (/\n#{1,6}\s/.test(trimmed)) return true;
  if (/\n[-*]\s+\S/.test(trimmed) && trimmed.includes("\n\n")) return true;
  return false;
}

/**
 * Return the plan markdown when this permission request is ExitPlanMode /
 * "Approve Plan". Returns `null` for ordinary tool permissions (edits, shell).
 */
export function extractPermissionPlanMarkdown(
  input: PermissionPlanInput,
): string | null {
  const explicit = trimPlan(input.planMarkdown);
  if (explicit) return explicit;

  const fromParams = planFromUnknown(input.params);
  if (fromParams) return fromParams;

  const fromDetail = planFromUnknown(input.detail);
  if (fromDetail) return fromDetail;

  const titleIsPlan = isPlanApprovalTitle(input.title);
  const optionsArePlan = optionsSuggestPlan(input.options);
  if ((titleIsPlan || optionsArePlan) && looksLikeMarkdown(input.detail)) {
    return trimPlan(input.detail);
  }

  return null;
}
