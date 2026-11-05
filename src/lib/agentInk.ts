import type { CSSProperties } from "react";

/**
 * Agent ink: each agent's one identity colour (docs/ui-language.md v2).
 * Components never branch on agent id for colour — they set `--agent-ink` on
 * the element with agentInkStyle() and CSS reads `var(--agent-ink, var(--accent))`.
 */
const INK_VAR: Record<string, string> = {
  "claude-code": "--agent-claude",
  "claude-acp": "--agent-claude",
  codex: "--agent-codex",
  "codex-acp": "--agent-codex",
  zcode: "--agent-zcode",
  "zcode-acp": "--agent-zcode",
  "grok-build": "--agent-grok",
  grok: "--agent-grok",
  deepseek: "--agent-deepseek",
  "deepseek-acp": "--agent-deepseek",
  "web-deepseek": "--agent-deepseek",
  opencode: "--agent-opencode",
  gemini: "--agent-gemini",
};

/** `var(--agent-…)` for an agent id; unknown and custom agents share the neutral ink. */
export function agentInkVar(agentId: string | null | undefined): string {
  const name = (agentId && INK_VAR[agentId.toLowerCase()]) || "--agent-other";
  return `var(${name})`;
}

/** Inline style that exposes the agent's ink to CSS as `--agent-ink`. */
export function agentInkStyle(agentId: string | null | undefined): CSSProperties {
  return { "--agent-ink": agentInkVar(agentId) } as CSSProperties;
}
