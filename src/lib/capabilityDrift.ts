import { normalizeAgentModeId } from "./acpSupplements";
import type { CapabilitySnapshot, SessionComposerPrefs } from "./types";

export type CapabilityDriftIssue = {
  field: "model" | "mode" | "effort" | "effortId";
  preferred: string;
  reason: string;
};

export type CapabilityDrift = {
  issues: CapabilityDriftIssue[];
  /** Prefs to write so disk no longer points at dead options (or remaps legacy ids). */
  clearedPrefs: SessionComposerPrefs;
  summary: string;
};

/**
 * Compare saved Composer prefs against live ACP caps after handshake /
 * agent update. Returns null when nothing is wrong (or prefs are empty).
 *
 * `agentId` lets us remap Codex legacy mode labels (full-auto → agent-full-access)
 * instead of wiping the user's full-access preference.
 */
export function detectCapabilityDrift(
  prefs: SessionComposerPrefs | null | undefined,
  caps: CapabilitySnapshot | null | undefined,
  agentId?: string | null,
): CapabilityDrift | null {
  if (!prefs || !caps) return null;

  const issues: CapabilityDriftIssue[] = [];
  const clearedPrefs: SessionComposerPrefs = {};

  const model = prefs.preferredModel?.trim() || null;
  if (model && caps.models.length > 0 && !caps.models.some((m) => m.id === model)) {
    issues.push({
      field: "model",
      preferred: model,
      reason: "no longer advertised",
    });
    clearedPrefs.preferredModel = null;
  }

  const modeRaw = prefs.preferredMode?.trim() || null;
  if (modeRaw && caps.modes.length > 0) {
    const mode = agentId ? normalizeAgentModeId(agentId, modeRaw) : modeRaw;
    if (caps.modes.some((m) => m.id === mode)) {
      // Keep preference; rewrite disk if the stored id was a legacy alias.
      if (mode !== modeRaw) {
        clearedPrefs.preferredMode = mode;
      }
    } else {
      issues.push({
        field: "mode",
        preferred: modeRaw,
        reason: "no longer advertised",
      });
      clearedPrefs.preferredMode = null;
    }
  }

  const effortId = prefs.preferredEffortId?.trim() || null;
  const hasEffortControl =
    Boolean(caps.effortConfigId) ||
    (caps.effortOptions?.length ?? 0) > 0 ||
    caps.thinkingEffort != null;

  // Effort levels are model-dependent (OpenCode: deepseek offers "max", many
  // others top out at "high"). Wiping the disk pref when the *current*
  // catalog lacks the level was wrong twice over:
  //   1) handshake still has the default model when a preferred model switch
  //      is pending — "max" vanished before deepseek was ever selected;
  //   2) after set_model, caps can lag a beat before the new thought levels
  //      land — same wipe, permanent "high" next launch.
  // Only clear when the agent has no effort control at all. A level that is
  // simply not on this model stays on disk so switching back can restore it.
  if (effortId && !hasEffortControl) {
    issues.push({
      field: "effortId",
      preferred: effortId,
      reason: "effort control removed",
    });
    clearedPrefs.preferredEffortId = null;
  }

  const effort = prefs.preferredEffort;
  if (typeof effort === "number" && Number.isFinite(effort) && !hasEffortControl) {
    issues.push({
      field: "effort",
      preferred: String(effort),
      reason: "effort control removed",
    });
    clearedPrefs.preferredEffort = null;
  }

  // Prefer rewriting legacy mode ids even when nothing is "broken".
  if (issues.length === 0 && Object.keys(clearedPrefs).length === 0) return null;

  const bits = issues.map((i) => `${i.field} “${i.preferred}” (${i.reason})`);
  return {
    issues,
    clearedPrefs,
    summary:
      issues.length === 0
        ? "Updated saved mode to the current agent id"
        : bits.length === 1
          ? `Saved ${bits[0]} — reset to agent default`
          : `Saved options no longer valid: ${bits.join("; ")}`,
  };
}
