import type { CapabilitySnapshot, SessionComposerPrefs } from "./types";

export type CapabilityDriftIssue = {
  field: "model" | "mode" | "effort" | "effortId";
  preferred: string;
  reason: string;
};

export type CapabilityDrift = {
  issues: CapabilityDriftIssue[];
  /** Prefs to write so disk no longer points at dead options. */
  clearedPrefs: SessionComposerPrefs;
  summary: string;
};

/**
 * Compare saved Composer prefs against live ACP caps after handshake /
 * agent update. Returns null when nothing is wrong (or prefs are empty).
 */
export function detectCapabilityDrift(
  prefs: SessionComposerPrefs | null | undefined,
  caps: CapabilitySnapshot | null | undefined,
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

  const mode = prefs.preferredMode?.trim() || null;
  if (mode && caps.modes.length > 0 && !caps.modes.some((m) => m.id === mode)) {
    issues.push({
      field: "mode",
      preferred: mode,
      reason: "no longer advertised",
    });
    clearedPrefs.preferredMode = null;
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

  if (issues.length === 0) return null;

  const bits = issues.map((i) => `${i.field} “${i.preferred}” (${i.reason})`);
  return {
    issues,
    clearedPrefs,
    summary:
      bits.length === 1
        ? `Saved ${bits[0]} — reset to agent default`
        : `Saved options no longer valid: ${bits.join("; ")}`,
  };
}
