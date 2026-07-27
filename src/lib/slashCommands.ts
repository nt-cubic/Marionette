import { getSessionUpdate, getSessionUpdateKind } from "./acpTranscript";
import { getAcpSupplement } from "./acpSupplements";
import type { AvailableCommand } from "./types";

/**
 * ACP agents MAY advertise slash commands via
 * `sessionUpdate: "available_commands_update"`.
 * Commands run as ordinary `session/prompt` text (`/name args…`).
 */

/** Static fallbacks when an agent never advertises (or ACP is still warming). */
const STATIC_SLASH: Record<string, AvailableCommand[]> = {
  "grok-build": [
    {
      name: "always-approve",
      description: "Toggle auto-approve for tool permissions (on/off)",
      input: { hint: "on | off" },
    },
  ],
  codex: [
    { name: "status", description: "Show account rate limits and session status" },
    { name: "model", description: "Switch model", input: { hint: "model id" } },
    {
      name: "approvals",
      description: "Change approval policy",
      input: { hint: "suggest | auto-edit | full-auto" },
    },
  ],
  "claude-code": [
    { name: "model", description: "Switch model", input: { hint: "alias" } },
    { name: "plan", description: "Enter plan mode" },
    { name: "effort", description: "Set thinking effort", input: { hint: "low | medium | high" } },
  ],
  opencode: [
    { name: "status", description: "Show session / provider status" },
  ],
};

export function staticSlashCommands(agentId: string): AvailableCommand[] {
  return STATIC_SLASH[agentId] ?? [];
}

/**
 * Prefer live ACP advertise; fall back to static + mode prompt commands so
 * Grok `/always-approve` etc. still complete before the agent speaks.
 */
export function resolveSlashCommands(
  agentId: string,
  advertised: AvailableCommand[] | null | undefined,
): AvailableCommand[] {
  if (advertised && advertised.length > 0) return advertised;

  const staticList = staticSlashCommands(agentId);
  const sup = getAcpSupplement(agentId);
  const fromMode: AvailableCommand[] = [];
  if (sup?.promptModeCommands) {
    for (const [modeId, cmd] of Object.entries(sup.promptModeCommands)) {
      const name = cmd.replace(/^\//, "").split(/\s+/)[0];
      if (!name) continue;
      if (staticList.some((c) => c.name === name) || fromMode.some((c) => c.name === name)) {
        continue;
      }
      fromMode.push({
        name,
        description: `Mode: ${modeId}`,
        input: cmd.includes(" ") ? { hint: cmd.slice(cmd.indexOf(" ") + 1) } : undefined,
      });
    }
  }
  return [...staticList, ...fromMode];
}

/** Parse `available_commands_update` from an ACP session/update payload. */
export function parseAvailableCommandsUpdate(data: unknown): AvailableCommand[] | null {
  const update = getSessionUpdate(data);
  if (!update) return null;
  const kind = getSessionUpdateKind(update);
  if (kind !== "available_commands_update" && kind !== "available_commands") {
    return null;
  }
  const raw =
    update.availableCommands ??
    update.available_commands ??
    update.commands;
  if (!Array.isArray(raw)) return [];
  const out: AvailableCommand[] = [];
  for (const item of raw) {
    if (!item || typeof item !== "object") continue;
    const rec = item as Record<string, unknown>;
    const name = typeof rec.name === "string" ? rec.name.trim() : "";
    if (!name) continue;
    const description =
      typeof rec.description === "string" ? rec.description : "";
    let input: AvailableCommand["input"];
    const inputRaw = rec.input;
    if (inputRaw && typeof inputRaw === "object" && !Array.isArray(inputRaw)) {
      const hint = (inputRaw as Record<string, unknown>).hint;
      if (typeof hint === "string" && hint.trim()) {
        input = { hint: hint.trim() };
      }
    }
    out.push({ name: name.replace(/^\//, ""), description, input });
  }
  return out;
}

/**
 * Active slash token at the caret when the draft is a single `/command…` line
 * (or the final line of a multi-line draft still starts with `/`).
 */
export function slashQueryAtCursor(
  draft: string,
  cursor: number,
): { query: string; start: number; end: number } | null {
  if (!draft.includes("/")) return null;
  const before = draft.slice(0, Math.max(0, cursor));
  // Only complete the command token on the current line.
  const lineStart = before.lastIndexOf("\n") + 1;
  const line = before.slice(lineStart);
  // Match `/cmd` or bare `/` at the start of the line (optional leading spaces).
  const m = line.match(/^(\s*)\/([^\s]*)$/);
  if (!m) return null;
  const lead = m[1].length;
  const start = lineStart + lead;
  return {
    query: m[2].toLowerCase(),
    start,
    end: cursor,
  };
}

export function filterSlashCommands(
  commands: AvailableCommand[],
  query: string,
): AvailableCommand[] {
  const q = query.toLowerCase();
  if (!q) return commands.slice(0, 12);
  return commands
    .filter(
      (c) =>
        c.name.toLowerCase().startsWith(q) ||
        c.name.toLowerCase().includes(q) ||
        c.description.toLowerCase().includes(q),
    )
    .slice(0, 12);
}

/** Insert `/name ` (or `/name ` with hint placeholder only as suffix space). */
export function applySlashCommand(
  draft: string,
  start: number,
  end: number,
  command: AvailableCommand,
): string {
  const insert = `/${command.name} `;
  return draft.slice(0, start) + insert + draft.slice(end);
}
