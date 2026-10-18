import type { Session } from "./types";
import {
  deserialize,
  emptyWorkspace,
  focusedSessionId,
  migrateFromFlat,
  openSessionIds as tabIdsFromTree,
  type SplitLayoutV1,
  type SplitWorkspaceState,
} from "./splitWorkspace";

const STORAGE_KEY = "marionette-ui-restore";
const SHELF_COLLAPSE_KEY = "marionette-project-shelf-collapsed";
const DETAILS_VISIBLE_KEY = "marionette-details-visible";

/** Last focused dialog + open tabs — restored on cold start. */
export type UiRestoreSnapshot = {
  sessionId: string;
  projectId: string;
  openSessionIds: string[];
  updatedAt: number;
  /** Optional pane tree. Missing = pre-split snapshot; restore via migrateFromFlat. */
  splitLayout?: SplitLayoutV1;
};

export type RestoredWorkspace = {
  session: Session;
  openSessionIds: string[];
  splitState: SplitWorkspaceState;
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
 * Clean View eye: closed = quiet folds (product default). Missing key is
 * closed so a first launch / cleared storage stays quiet. Open is remembered.
 */
export function loadDetailsVisible(): boolean {
  try {
    const raw = window.localStorage.getItem(DETAILS_VISIBLE_KEY);
    if (raw == null) return false;
    return raw === "1";
  } catch {
    return false;
  }
}

export function saveDetailsVisible(visible: boolean): void {
  try {
    window.localStorage.setItem(DETAILS_VISIBLE_KEY, visible ? "1" : "0");
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
  splitLayout?: SplitLayoutV1;
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
    if (snap.splitLayout?.version === 1) payload.splitLayout = snap.splitLayout;
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
    effortId?: string | null;
    effort?: number | null;
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

export function parseUiRestore(raw: unknown): UiRestoreSnapshot | null {
  if (!raw || typeof raw !== "object") return null;
  const rec = raw as Record<string, unknown>;
  if (typeof rec.sessionId !== "string" || !rec.sessionId.trim()) return null;
  const openSessionIds = Array.isArray(rec.openSessionIds)
    ? rec.openSessionIds.filter((id): id is string => typeof id === "string" && id.trim() !== "")
    : [];
  const snap: UiRestoreSnapshot = {
    sessionId: rec.sessionId.trim(),
    projectId: typeof rec.projectId === "string" ? rec.projectId.trim() : "",
    openSessionIds,
    updatedAt: typeof rec.updatedAt === "number" ? rec.updatedAt : 0,
  };
  if (rec.splitLayout && typeof rec.splitLayout === "object") {
    const layout = rec.splitLayout as Record<string, unknown>;
    if (layout.version === 1) snap.splitLayout = rec.splitLayout as SplitLayoutV1;
  }
  return snap;
}

export function loadUiRestore(): UiRestoreSnapshot | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    return parseUiRestore(JSON.parse(raw));
  } catch {
    return null;
  }
}

/**
 * Rebuild the pane tree from a snapshot. Detached URL wins and ignores the
 * saved tree. Bad / empty trees fall back to the flat tab list.
 */
export function restoreSplitFromSnapshot(
  saved: UiRestoreSnapshot | null,
  knownSessionIds: ReadonlySet<string>,
  opts?: {
    maxLeaves?: number;
    excludeSessionIds?: ReadonlySet<string>;
    detachedSessionId?: string | null;
  },
): SplitWorkspaceState {
  const detachedId = opts?.detachedSessionId?.trim();
  if (detachedId && knownSessionIds.has(detachedId)) {
    return migrateFromFlat([detachedId], detachedId);
  }
  if (!saved) return emptyWorkspace();
  const exclude = opts?.excludeSessionIds ?? new Set<string>();
  const maxLeaves = opts?.maxLeaves;
  if (saved.splitLayout) {
    const tree = deserialize(saved.splitLayout, knownSessionIds, {
      maxLeaves,
      excludeSessionIds: exclude,
    });
    if (tabIdsFromTree(tree).length > 0) return tree;
  }
  const tabs = saved.openSessionIds.filter((id) => knownSessionIds.has(id) && !exclude.has(id));
  const focus =
    knownSessionIds.has(saved.sessionId) && !exclude.has(saved.sessionId)
      ? saved.sessionId
      : (tabs[0] ?? "");
  return migrateFromFlat(tabs, focus);
}

/**
 * Choose the session to focus after loading all project sessions.
 * Priority: detached URL → last UI focus (pane tree if present). No snapshot
 * (first launch / cleared storage / stale ids) leaves the workspace empty.
 */
export function pickRestoredSession(
  loaded: Session[],
  opts?: {
    detachedSessionId?: string | null;
    excludeSessionIds?: ReadonlySet<string>;
    maxLeaves?: number;
    snapshot?: UiRestoreSnapshot | null;
  },
): RestoredWorkspace | null {
  if (loaded.length === 0) return null;
  const byId = new Map(loaded.map((s) => [s.id, s]));
  const known = new Set(byId.keys());

  const detachedId = opts?.detachedSessionId?.trim();
  if (detachedId) {
    const det = byId.get(detachedId);
    if (det) {
      return {
        session: det,
        openSessionIds: [det.id],
        splitState: migrateFromFlat([det.id], det.id),
      };
    }
  }

  const saved =
    opts && Object.prototype.hasOwnProperty.call(opts, "snapshot")
      ? (opts.snapshot ?? null)
      : loadUiRestore();
  const tree = restoreSplitFromSnapshot(saved, known, opts);
  const ids = tabIdsFromTree(tree);
  if (ids.length === 0) return null;
  const focusId = focusedSessionId(tree) || ids[0];
  const session = byId.get(focusId) ?? byId.get(ids[0]);
  if (!session) return null;
  return { session, openSessionIds: ids, splitState: tree };
}
