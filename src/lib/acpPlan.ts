import { getSessionUpdate, getSessionUpdateKind } from "./acpTranscript";

/**
 * ACP `sessionUpdate: "plan"` entry — agent-maintained task list for the turn.
 * Full-replace semantics: each update is the entire table, not a delta.
 */
export type PlanEntry = {
  content: string;
  status: "pending" | "in_progress" | "completed";
  priority?: "high" | "medium" | "low";
};

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function normalizeStatus(raw: unknown): PlanEntry["status"] {
  const s = typeof raw === "string" ? raw.trim().toLowerCase().replace(/-/g, "_") : "";
  if (s === "in_progress" || s === "inprogress" || s === "doing" || s === "active") {
    return "in_progress";
  }
  if (s === "completed" || s === "complete" || s === "done" || s === "finished") {
    return "completed";
  }
  return "pending";
}

function normalizePriority(raw: unknown): PlanEntry["priority"] | undefined {
  const s = typeof raw === "string" ? raw.trim().toLowerCase() : "";
  if (s === "high" || s === "medium" || s === "low") return s;
  return undefined;
}

/**
 * Parse `plan` from an ACP session/update payload.
 * Returns `null` when this update is not a plan (same contract as
 * `parseAvailableCommandsUpdate`). Returns `[]` for an empty plan wipe.
 */
export function parseAcpPlanUpdate(data: unknown): PlanEntry[] | null {
  const update = getSessionUpdate(data);
  if (!update) return null;
  const kind = getSessionUpdateKind(update);
  if (kind !== "plan" && kind !== "plan_update") {
    return null;
  }
  const raw =
    update.entries ??
    update.plan ??
    update.items ??
    update.tasks;
  if (!Array.isArray(raw)) return [];
  const out: PlanEntry[] = [];
  for (const item of raw) {
    const rec = asRecord(item);
    if (!rec) continue;
    const content =
      (typeof rec.content === "string" && rec.content.trim()) ||
      (typeof rec.text === "string" && rec.text.trim()) ||
      (typeof rec.title === "string" && rec.title.trim()) ||
      (typeof rec.description === "string" && rec.description.trim()) ||
      "";
    if (!content) continue;
    const entry: PlanEntry = {
      content,
      status: normalizeStatus(rec.status ?? rec.state),
    };
    const priority = normalizePriority(rec.priority);
    if (priority) entry.priority = priority;
    out.push(entry);
  }
  return out;
}
