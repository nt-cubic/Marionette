import type { Session } from "./types";

const STORAGE_KEY = "marionette-ui-restore";
const SHELF_COLLAPSE_KEY = "marionette-project-shelf-collapsed";

/** Last focused dialog + open tabs — restored on cold start. */
export type UiRestoreSnapshot = {
  sessionId: string;
  projectId: string;
  openSessionIds: string[];
  updatedAt: number;
};

/**
 * Project ids the user collapsed in the left shelf.
 * Missing ids default to expanded. New projects start expanded.
 */
export function loadCollapsedProjectIds(): Set<string> {
  try {
    const raw = window.localStorage.getItem(SHELF_COLLAPSE_KEY);
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(
      parsed.filter((id): id is string => typeof id === "string" && id.trim() !== "")
    );
  } catch {
    return new Set();
  }
}

export function saveCollapsedProjectIds(ids: Set<string> | Iterable<string>): void {
  try {
    const list = [...ids].filter((id) => id.trim() !== "");
    window.localStorage.setItem(SHELF_COLLAPSE_KEY, JSON.stringify(list));
  } catch {
    // localStorage might be disabled or full
  }
}

/**
 * Remember which dialog the user was looking at (and which tabs were open).
 * Session agent / model / mode / effort already live on the session row on disk;
 * this only picks *which* row to focus after relaunch.
 */
export function saveUiRestore(snap: {
  sessionId: string;
  projectId: string;
  openSessionIds: string[];
}): void {
  const sessionId = snap.sessionId?.trim() ?? "";
  if (!sessionId || sessionId.startsWith("session-empty-")) return;
  try {
    const openSessionIds = (snap.openSessionIds ?? [])
      .map((id) => id.trim())
      .filter((id) => id && !id.startsWith("session-empty-"));
    const payload: UiRestoreSnapshot = {
      sessionId,
      projectId: snap.projectId?.trim() ?? "",
      openSessionIds: openSessionIds.includes(sessionId)
        ? openSessionIds
        : [sessionId, ...openSessionIds],
      updatedAt: Date.now(),
    };
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // localStorage might be disabled or full
  }
}

const QUEUED_SENDS_KEY = "marionette-queued-sends";
const CLOSED_TABS_KEY = "marionette-closed-tabs";

export type QueuedSendSnap = {
  composed: string;
  imageAttachments: unknown[];
  composerSnap?: {
    modeId?: string | null;
    modeLabel?: string | null;
    modelId?: string | null;
    modelLabel?: string | null;
    effortLabel?: string | null;
  };
};

export function saveQueuedSends(map: Record<string, QueuedSendSnap[]>): void {
  try {
    const trimmed: Record<string, QueuedSendSnap[]> = {};
    for (const [id, items] of Object.entries(map)) {
      if (items.length > 0) trimmed[id] = items;
    }
    window.localStorage.setItem(QUEUED_SENDS_KEY, JSON.stringify(trimmed));
  } catch {
    /* ignore */
  }
}

export function loadQueuedSends(): Record<string, QueuedSendSnap[]> {
  try {
    const raw = window.localStorage.getItem(QUEUED_SENDS_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    const out: Record<string, QueuedSendSnap[]> = {};
    for (const [id, items] of Object.entries(parsed as Record<string, unknown>)) {
      if (!Array.isArray(items)) continue;
      out[id] = items.filter(
        (item): item is QueuedSendSnap =>
          Boolean(item) && typeof item === "object" && typeof (item as QueuedSendSnap).composed === "string",
      );
    }
    return out;
  } catch {
    return {};
  }
}

export function saveClosedTabs(ids: string[]): void {
  try {
    window.localStorage.setItem(CLOSED_TABS_KEY, JSON.stringify(ids.slice(0, 20)));
  } catch {
    /* ignore */
  }
}

export function loadClosedTabs(): string[] {
  try {
    const raw = window.localStorage.getItem(CLOSED_TABS_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((id): id is string => typeof id === "string" && id.trim() !== "");
  } catch {
    return [];
  }
}

export function loadUiRestore(): UiRestoreSnapshot | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return null;
    const rec = parsed as Record<string, unknown>;
    if (typeof rec.sessionId !== "string" || !rec.sessionId.trim()) return null;
    const openSessionIds = Array.isArray(rec.openSessionIds)
      ? rec.openSessionIds.filter((id): id is string => typeof id === "string" && id.trim() !== "")
      : [];
    return {
      sessionId: rec.sessionId.trim(),
      projectId: typeof rec.projectId === "string" ? rec.projectId.trim() : "",
      openSessionIds,
      updatedAt: typeof rec.updatedAt === "number" ? rec.updatedAt : 0,
    };
  } catch {
    return null;
  }
}

/**
 * Choose the session to focus after loading all project sessions.
 * Priority: detached URL → last UI focus → newest lastActiveAt on disk.
 */
export function pickRestoredSession(
  loaded: Session[],
  opts?: { detachedSessionId?: string | null },
): { session: Session; openSessionIds: string[] } | null {
  if (loaded.length === 0) return null;
  const byId = new Map(loaded.map((s) => [s.id, s]));

  const detachedId = opts?.detachedSessionId?.trim();
  if (detachedId) {
    const det = byId.get(detachedId);
    if (det) return { session: det, openSessionIds: [det.id] };
  }

  const saved = loadUiRestore();
  if (saved) {
    const session = byId.get(saved.sessionId);
    if (session) {
      const open = saved.openSessionIds.filter((id) => byId.has(id));
      if (!open.includes(session.id)) open.unshift(session.id);
      return {
        session,
        openSessionIds: open.length > 0 ? open : [session.id],
      };
    }
  }

  // Fallback: most recently touched session on disk (prefs / agent / send update lastActiveAt).
  const sorted = [...loaded].sort((a, b) => {
    const tb = Date.parse(b.lastActiveAt) || 0;
    const ta = Date.parse(a.lastActiveAt) || 0;
    return tb - ta;
  });
  const session = sorted[0];
  return { session, openSessionIds: [session.id] };
}
