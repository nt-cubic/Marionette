import type { CapabilitySnapshot, SessionComposerPrefs } from "./types";

/**
 * Offline cache of ACP capability catalogs (models / modes / effort).
 *
 * Composer controls are a product surface: they must be on screen the moment a
 * dialog opens, not only after a handshake that can take ~10s. What an agent
 * advertises is agent-scoped and stable between runs, so the last negotiated
 * snapshot is kept per agent id and every ACP connect is treated as a refresh
 * of that cache — never as the thing that creates the controls.
 *
 * HARD RULE stays intact: the cache key is the agent id, so a dialog can never
 * render another agent's models. Per-dialog selections still come from the
 * session prefs on disk (see `overlayPrefs`).
 */

const KEY_PREFIX = "agentshell-caps:";
/** Bump when CapabilitySnapshot shape changes in a breaking way. */
const VERSION = 2;
/** Forget catalogs nobody has refreshed in a month (agent uninstalled / renamed). */
const MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;

type CacheEntry = {
  version: number;
  savedAt: number;
  snapshot: CapabilitySnapshot;
  /** CLI `--version` when this catalog was written — used to detect agent updates. */
  agentVersion?: string | null;
};

function keyFor(agentId: string): string {
  return `${KEY_PREFIX}${agentId}`;
}

function readEntry(agentId: string): CacheEntry | null {
  if (!agentId) return null;
  try {
    const raw = window.localStorage.getItem(keyFor(agentId));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CacheEntry;
    if (!parsed || parsed.version !== VERSION || !parsed.snapshot) return null;
    if (!Number.isFinite(parsed.savedAt) || Date.now() - parsed.savedAt > MAX_AGE_MS) {
      window.localStorage.removeItem(keyFor(agentId));
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

/** Fill in fields older snapshots (or event-bridge edge cases) may be missing. */
function normalize(snapshot: CapabilitySnapshot): CapabilitySnapshot {
  return {
    ...snapshot,
    modes: snapshot.modes ?? [],
    models: snapshot.models ?? [],
    effortOptions: snapshot.effortOptions ?? [],
    thinkingEffort: snapshot.thinkingEffort ?? null,
    supportsCancel: snapshot.supportsCancel ?? true,
    currentMode: snapshot.currentMode ?? null,
    currentModel: snapshot.currentModel ?? null,
    currentEffort: snapshot.currentEffort ?? null,
    currentEffortId: snapshot.currentEffortId ?? null,
    modelConfigId: snapshot.modelConfigId ?? null,
    modeConfigId: snapshot.modeConfigId ?? null,
    effortConfigId: snapshot.effortConfigId ?? null,
  };
}

/**
 * Store the catalog for this agent.
 *
 * `handshake` marks a snapshot that came straight from `session/new` — only
 * those carry the agent's own defaults, so a later `set_config` echo refreshes
 * the catalog without rewriting what "default" means for a fresh dialog.
 */
export function cacheAgentCapabilities(
  agentId: string,
  caps: CapabilitySnapshot | null | undefined,
  opts: { handshake: boolean; agentVersion?: string | null },
): void {
  if (!agentId || !caps) return;
  const next = normalize(caps);
  // An empty catalog is not worth remembering — it would only blank the chips.
  if (next.models.length === 0 && next.modes.length === 0) return;

  const previousEntry = readEntry(agentId);
  const previous = previousEntry?.snapshot;
  const keepDefaults = !opts.handshake && previous != null;
  const snapshot: CapabilitySnapshot = keepDefaults
    ? {
        ...next,
        currentModel: previous.currentModel,
        currentMode: previous.currentMode,
        currentEffort: previous.currentEffort,
        currentEffortId: previous.currentEffortId,
      }
    : next;

  const agentVersion =
    opts.agentVersion !== undefined
      ? opts.agentVersion
      : previousEntry?.agentVersion ?? null;

  const entry: CacheEntry = {
    version: VERSION,
    savedAt: Date.now(),
    snapshot,
    agentVersion,
  };
  try {
    window.localStorage.setItem(keyFor(agentId), JSON.stringify(entry));
  } catch {
    // Quota / private mode: the UI just falls back to live-only capabilities.
  }
}

/** CLI version that last wrote this agent's catalog (if known). */
export function cachedAgentVersion(agentId: string): string | null {
  return readEntry(agentId)?.agentVersion ?? null;
}

/**
 * Drop the offline catalog when the installed CLI version changed so Composer
 * cannot paint stale model/mode/effort chips after an agent update.
 */
export function invalidateCapsIfAgentUpdated(
  agentId: string,
  installedVersion: string | null | undefined,
): boolean {
  if (!agentId || !installedVersion) return false;
  const entry = readEntry(agentId);
  if (!entry) return false;
  const prev = entry.agentVersion?.trim() || null;
  if (!prev || prev === installedVersion.trim()) return false;
  try {
    window.localStorage.removeItem(keyFor(agentId));
    return true;
  } catch {
    return false;
  }
}

/** Force-drop a catalog (e.g. after npm install of the agent). */
export function clearAgentCapabilities(agentId: string): void {
  if (!agentId) return;
  try {
    window.localStorage.removeItem(keyFor(agentId));
  } catch {
    // ignore
  }
}

/** Per-dialog selections win over the agent-wide defaults in the cache. */
function overlayPrefs(
  snapshot: CapabilitySnapshot,
  prefs: SessionComposerPrefs | null | undefined,
): CapabilitySnapshot {
  if (!prefs) return snapshot;
  const model = prefs.preferredModel?.trim() || null;
  const mode = prefs.preferredMode?.trim() || null;
  const effortId = prefs.preferredEffortId?.trim() || null;
  const effort =
    typeof prefs.preferredEffort === "number" && Number.isFinite(prefs.preferredEffort)
      ? prefs.preferredEffort
      : null;
  return {
    ...snapshot,
    // Only restore a saved choice the agent still advertises.
    currentModel:
      model && (snapshot.models.length === 0 || snapshot.models.some((m) => m.id === model))
        ? model
        : snapshot.currentModel,
    currentMode:
      mode && (snapshot.modes.length === 0 || snapshot.modes.some((m) => m.id === mode))
        ? mode
        : snapshot.currentMode,
    currentEffortId:
      effortId && snapshot.effortOptions.some((o) => o.id === effortId)
        ? effortId
        : snapshot.currentEffortId,
    currentEffort: effort ?? snapshot.currentEffort,
  };
}

/**
 * Last known controls for (agent, dialog) — good enough to render before the
 * agent process exists. Returns null when this agent has never connected.
 */
export function cachedCapabilitiesFor(
  agentId: string,
  prefs?: SessionComposerPrefs | null,
): CapabilitySnapshot | null {
  const entry = readEntry(agentId);
  if (!entry) return null;
  return overlayPrefs(normalize(entry.snapshot), prefs);
}
