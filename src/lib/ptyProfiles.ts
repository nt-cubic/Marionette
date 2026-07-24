import type { CapabilitySnapshot, ModeDef, ModelDef, ThinkingEffort } from "./types";

/**
 * Static control surface for PTY agents (no ACP capability negotiation).
 * Selecting model/mode/effort injects slash commands into the live TUI.
 *
 * Commands are best-effort: each CLI evolves; wrong slash still beats no UI.
 */
export type PtyControlProfile = {
  agentId: string;
  models: ModelDef[];
  modes: ModeDef[];
  thinkingEffort: ThinkingEffort | null;
  defaultModel?: string;
  defaultMode?: string;
  /** Build a line to send to PTY stdin (without trailing CR). */
  commandForModel?: (modelId: string) => string | null;
  commandForMode?: (modeId: string) => string | null;
  /** Map 0..1 effort slider to a TUI command. */
  commandForEffort?: (value: number) => string | null;
};

function effortLevel(value: number): "low" | "medium" | "high" {
  if (value <= 0.25) return "low";
  if (value >= 0.75) return "high";
  return "medium";
}

const CLAUDE: PtyControlProfile = {
  agentId: "claude-code",
  models: [
    { id: "sonnet", label: "Sonnet" },
    { id: "opus", label: "Opus" },
    { id: "haiku", label: "Haiku" },
    { id: "fable", label: "Fable" },
  ],
  modes: [
    { id: "default", label: "Default" },
    { id: "plan", label: "Plan" },
    { id: "acceptEdits", label: "Accept edits" },
  ],
  thinkingEffort: { min: 0, max: 1, default: 0.5 },
  defaultModel: "sonnet",
  defaultMode: "default",
  commandForModel: (id) => `/model ${id}`,
  commandForMode: (id) => {
    if (id === "default") return null;
    if (id === "plan") return "/plan";
    if (id === "acceptEdits") return "/permissions";
    return `/${id}`;
  },
  commandForEffort: (value) => `/effort ${effortLevel(value)}`,
};

const CODEX: PtyControlProfile = {
  agentId: "codex",
  models: [
    { id: "gpt-5.2-codex", label: "GPT-5.2 Codex" },
    { id: "gpt-5.1-codex", label: "GPT-5.1 Codex" },
    { id: "o3", label: "o3" },
    { id: "o4-mini", label: "o4-mini" },
  ],
  modes: [
    { id: "default", label: "Default" },
    { id: "suggest", label: "Suggest" },
    { id: "auto-edit", label: "Auto edit" },
    { id: "full-auto", label: "Full auto" },
  ],
  thinkingEffort: { min: 0, max: 1, default: 0.5 },
  defaultModel: "gpt-5.2-codex",
  defaultMode: "default",
  commandForModel: (id) => `/model ${id}`,
  // Approval modes are often set at launch; still offer common slash attempts.
  commandForMode: (id) => (id === "default" ? null : `/approvals ${id}`),
  commandForEffort: (value) => `/reasoning ${effortLevel(value)}`,
};

// Grok Build product path is ACP: `grok agent stdio` (see models.rs).
// No PTY profile — capabilities come from ACP negotiation like OpenCode.

const PROFILES: Record<string, PtyControlProfile> = {
  "claude-code": CLAUDE,
  codex: CODEX,
};

export function getPtyProfile(agentId: string): PtyControlProfile | null {
  return PROFILES[agentId] ?? null;
}

export function ptyProfileToCapabilities(agentId: string): CapabilitySnapshot | null {
  const p = getPtyProfile(agentId);
  if (!p) return null;
  return {
    modes: p.modes,
    models: p.models,
    thinkingEffort: p.thinkingEffort,
    effortOptions: [],
    supportsCancel: false,
    currentMode: p.defaultMode ?? p.modes[0]?.id ?? null,
    currentModel: p.defaultModel ?? p.models[0]?.id ?? null,
    currentEffort: p.thinkingEffort?.default ?? null,
    currentEffortId: null,
    modelConfigId: p.commandForModel ? "model" : null,
    modeConfigId: p.commandForMode ? "mode" : null,
    effortConfigId: p.commandForEffort ? "effort" : null,
  };
}

/** Build slash line(s) for a Composer patch. Returns null if nothing to send. */
export function ptyCommandsForPatch(
  agentId: string,
  patch: Record<string, unknown>,
): string[] {
  const p = getPtyProfile(agentId);
  if (!p) return [];
  const out: string[] = [];

  if (typeof patch.model === "string" && p.commandForModel) {
    const line = p.commandForModel(patch.model);
    if (line) out.push(line);
  }
  if (typeof patch.mode === "string" && p.commandForMode) {
    const line = p.commandForMode(patch.mode);
    if (line) out.push(line);
  }
  if (typeof patch.thinkingEffort === "number" && p.commandForEffort) {
    const line = p.commandForEffort(patch.thinkingEffort);
    if (line) out.push(line);
  }
  // Also accept configId form from future UI
  if (typeof patch.configId === "string" && patch.value != null) {
    const v = String(patch.value);
    if (patch.configId === "model" && p.commandForModel) {
      const line = p.commandForModel(v);
      if (line) out.push(line);
    }
    if (patch.configId === "mode" && p.commandForMode) {
      const line = p.commandForMode(v);
      if (line) out.push(line);
    }
  }

  return out;
}
