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
  // Account-scoped ids (`account:<plan>\<model>`, zcode) live and die with the
  // spawn's credential availability: a fresh ACP process enumerates only the
  // accounts whose credentials were readable at session/new, so one spawn can
  // advertise fewer accounts than the last (2026-11-21: a restart came up
  // without the Start Plan account, the drift wipe rewrote the dialog onto the
  // empty-balance Individual account and every turn 1113'd). A missing account
  // is a transient, not a dead preference — keep it on disk so the next spawn
  // that advertises it again restores the user's choice.
  const accountScoped = model?.startsWith("account:") ?? false;
  // Bridge wire ids (`marionette/â¦`, `marionette::â¦`, `marionette\â¦`) are also
  // spawn-conditional: the provider is injected only when the engine happens to
  // be running at spawn (engine asleep / still starting => no provider => the
  // whole group is absent). Treating that as a dead preference rewrote the
  // user's shared-model pick to the harness default (OpenCode's
  // seaglass-alpha-free) on the first cold start (2026-11-27). Keep it on disk.
  const bridgeScoped = isBridgeWireModelId(model);
  if (
    model &&
    !accountScoped &&
    !bridgeScoped &&
    caps.models.length > 0 &&
    !caps.models.some((m) => m.id === model)
  ) {
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

/**
 * True when a saved **account-scoped** model pref (`account:…`, zcode) is not
 * advertised by this spawn's catalog. That is a spawn-transient account drop
 * (the agent enumerates accounts per credential availability), not a dead
 * preference: the pref must survive on disk, the send must not silently
 * re-point at the agent default's account, and the wire must fail closed
 * rather than bill an account the user did not choose.
 */
const BRIDGE_WIRE_PREFIXES = ["marionette/", "marionette::", "marionette\\"];

/** True for the agent-native wire ids the bridge injects into a catalog. */
export function isBridgeWireModelId(model: string | null | undefined): boolean {
  const id = model?.trim() || "";
  return BRIDGE_WIRE_PREFIXES.some((prefix) => id.startsWith(prefix));
}

/**
 * Same spawn-transient story for bridge wire ids: when the engine was asleep
 * at spawn the provider is absent from this process' catalog even though the
 * next spawn injects it again. Used to keep a fallback chip from rewriting the
 * saved pick on disk (the chip still rides the wire honestly).
 */
export function isBridgePrefTransientlyMissing(
  savedModel: string | null | undefined,
  live: CapabilitySnapshot | null | undefined,
): boolean {
  const model = savedModel?.trim() || null;
  if (!model || !isBridgeWireModelId(model)) return false;
  if (!live || (live.models?.length ?? 0) === 0) return false;
  return !live.models.some((m) => m.id === model);
}

export function isAccountPrefTransientlyMissing(
  savedModel: string | null | undefined,
  live: CapabilitySnapshot | null | undefined,
): boolean {
  const model = savedModel?.trim() || null;
  if (!model || !model.startsWith("account:")) return false;
  if (!live || (live.models?.length ?? 0) === 0) return false;
  return !live.models.some((m) => m.id === model);
}
