/**
 * Module-level cache for agent CLI versions / update flags.
 *
 * Composer remounts on every dialog/agent switch, so keeping this state only in
 * React meant reopening the agent menu re-probed every time. A small process
 * cache makes installed versions and "update available" stick for the life of
 * the app, with a quiet background refresh.
 */

import { agentVersions, isTauriRuntime } from "./api";
import type { AgentVersionInfo } from "./types";

/** How often we re-hit the npm registry for "is there an update". */
const REGISTRY_TTL_MS = 30 * 60 * 1000;

type CacheState = {
  byId: Record<string, AgentVersionInfo>;
  /** Wall clock of the last successful registry (checkRegistry=true) pass. */
  registryAt: number;
  /** Wall clock of the last local `--version` pass. */
  localAt: number;
};

const state: CacheState = {
  byId: {},
  registryAt: 0,
  localAt: 0,
};

type Listener = (byId: Record<string, AgentVersionInfo>) => void;
const listeners = new Set<Listener>();

let inflightLocal: Promise<Record<string, AgentVersionInfo>> | null = null;
let inflightRegistry: Promise<Record<string, AgentVersionInfo>> | null = null;

function snapshot(): Record<string, AgentVersionInfo> {
  return { ...state.byId };
}

function publish() {
  const next = snapshot();
  for (const listener of listeners) {
    try {
      listener(next);
    } catch {
      // UI listeners must not break each other.
    }
  }
}

function mergeList(list: AgentVersionInfo[]) {
  if (list.length === 0) return;
  const next = { ...state.byId };
  for (const info of list) {
    const prev = next[info.id];
    // A local-only pass leaves `latest` empty — keep any earlier registry answer
    // so the update badge does not flicker off while we re-probe.
    if (prev && !info.latest && prev.latest) {
      next[info.id] = {
        ...info,
        latest: prev.latest,
        updateAvailable:
          info.installed && prev.latest
            ? // Recompute from what we still know, in case installed advanced.
              isNewerCached(prev.latest, info.installed)
            : prev.updateAvailable && info.installed === prev.installed
              ? prev.updateAvailable
              : false,
        note: info.note ?? prev.note,
      };
    } else {
      next[info.id] = info;
    }
  }
  state.byId = next;
  publish();
}

/** Numeric dotted compare — mirrors Rust `is_newer` for the merge path only. */
function isNewerCached(candidate: string, current: string): boolean {
  const core = (v: string) =>
    (v.split(/[-+]/)[0] ?? v).split(".").map((part) => {
      const n = Number.parseInt(part, 10);
      return Number.isFinite(n) ? n : 0;
    });
  const a = core(candidate);
  const b = core(current);
  const len = Math.max(a.length, b.length);
  for (let i = 0; i < len; i++) {
    const x = a[i] ?? 0;
    const y = b[i] ?? 0;
    if (x !== y) return x > y;
  }
  return !candidate.includes("-") && current.includes("-");
}

/** Immediate snapshot for first paint (may be empty until the first refresh). */
export function getCachedAgentVersions(): Record<string, AgentVersionInfo> {
  return snapshot();
}

export function subscribeAgentVersions(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Refresh installed versions (always cheap) and, when stale or forced, the
 * registry "latest" field. Concurrent callers share one in-flight pass.
 */
export async function refreshAgentVersions(options?: {
  /** Skip the TTL and re-query npm. */
  forceRegistry?: boolean;
  /** Skip network entirely — local `--version` only. */
  localOnly?: boolean;
}): Promise<Record<string, AgentVersionInfo>> {
  if (!isTauriRuntime()) return snapshot();

  const forceRegistry = options?.forceRegistry === true;
  const localOnly = options?.localOnly === true;

  if (!inflightLocal) {
    inflightLocal = (async () => {
      const list = await agentVersions(false);
      state.localAt = Date.now();
      mergeList(list);
      return snapshot();
    })().finally(() => {
      inflightLocal = null;
    });
  }
  await inflightLocal;

  if (localOnly) return snapshot();

  const registryStale =
    forceRegistry || Date.now() - state.registryAt > REGISTRY_TTL_MS;
  if (!registryStale) return snapshot();

  if (!inflightRegistry) {
    inflightRegistry = (async () => {
      const list = await agentVersions(true);
      state.registryAt = Date.now();
      // Full registry answers replace the merge (they already include installed).
      if (list.length > 0) {
        state.byId = Object.fromEntries(list.map((v) => [v.id, v]));
        publish();
      }
      return snapshot();
    })().finally(() => {
      inflightRegistry = null;
    });
  }
  return inflightRegistry;
}

/** After a successful install/upgrade — drop "update available" for that agent. */
export function patchAgentVersion(
  agentId: string,
  patch: Partial<Omit<AgentVersionInfo, "id">>
): void {
  const prev = state.byId[agentId];
  state.byId = {
    ...state.byId,
    [agentId]: {
      id: agentId,
      package: prev?.package ?? null,
      installed: prev?.installed ?? null,
      latest: prev?.latest ?? null,
      updateAvailable: false,
      note: prev?.note ?? null,
      ...patch,
    },
  };
  publish();
}
