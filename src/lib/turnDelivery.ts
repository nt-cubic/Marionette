import type { SessionEvent } from "./types";
import { t } from "./i18n";
import { classifyToolCall, extractDiffStats, formatActivityTally } from "./turnActivity";

/**
 * Per-turn delivery summary for the end-of-turn card: which files the turn
 * touched (git truth + tool edits union), how many commands ran, and the
 * activity tally. Pure data — derived on every render, never persisted.
 */

export type TurnDeliveryChangeType = "added" | "modified" | "deleted" | "untracked";

export type TurnDeliveryFile = {
  /** Path as the tool / git reported it; relative when git knew the file. */
  path: string;
  /** Path relative to the session cwd; full path when it is not under cwd. */
  relPath: string;
  /** From turn-end git truth (`file_change`); absent in non-git projects. */
  changeType?: TurnDeliveryChangeType;
  /**
   * Line counts: git's `--numstat` for the turn window when the file came back
   * from the workspace delta, otherwise what this turn's edit/write tool calls
   * reported. null = unknown.
   */
  add: number | null;
  del: number | null;
  /** How many edit/write tool calls touched this file in the turn. */
  touched: number;
};

/** One path of the turn's git delta (`diff-tree` between the two snapshots). */
export type TurnWorkspaceChange = {
  path: string;
  changeType: TurnDeliveryChangeType;
  add: number | null;
  del: number | null;
  binary?: boolean;
};

/**
 * Normalize a path for set comparison against `diff-tree` output: forward
 * slashes, project-relative, lowercased (Windows worktree).
 */
export function normalizeWorkspacePath(path: string, root: string): string {
  const pathFwd = path.replace(/\\/g, "/").replace(/^(\.\/)+/, "");
  const rootFwd = (root || "").replace(/\\/g, "/").replace(/\/+$/, "");
  if (!rootFwd) return pathFwd.toLowerCase();
  const lowerPath = pathFwd.toLowerCase();
  const lowerRoot = rootFwd.toLowerCase();
  if (lowerPath === lowerRoot) return "";
  if (lowerPath.startsWith(`${lowerRoot}/`)) {
    return pathFwd.slice(rootFwd.length + 1).toLowerCase();
  }
  return pathFwd.toLowerCase();
}

/**
 * Which workspace changes may be attributed to this turn.
 *
 * A path that was **clean** when the turn started (absent from `dirtyAtStart`)
 * and dirty at the end is always the turn's: any writer counts — an edit tool,
 * a subagent, `npm run build`, a formatter.
 *
 * A path that was **already dirty** at turn start is only the turn's when the
 * turn's own tools touched it. Without that rule, edits made by whatever else
 * shares the workspace (a second Marionette conversation on the same project, a
 * dev server, the user's editor) are published as this turn's work — the
 * "delivery card shows the whole repo" complaint (2026-10-23).
 *
 * `dirtyAtStart === null` (repo without HEAD) means the split is unknowable, so
 * every change in the window is kept rather than silently dropping files.
 */
export function selectTurnWorkspaceChanges(
  changes: TurnWorkspaceChange[],
  options: { dirtyAtStart: Iterable<string> | null; touched: Iterable<string>; root?: string },
): TurnWorkspaceChange[] {
  const root = options.root ?? "";
  if (options.dirtyAtStart === null) return changes;
  const dirty = new Set<string>();
  for (const path of options.dirtyAtStart) dirty.add(normalizeWorkspacePath(path, root));
  const touched = new Set<string>();
  for (const path of options.touched) touched.add(normalizeWorkspacePath(path, root));
  return changes.filter((change) => {
    const key = normalizeWorkspacePath(change.path, root);
    return !dirty.has(key) || touched.has(key);
  });
}

export type TurnDelivery = {
  files: TurnDeliveryFile[];
  totalDiff: { add: number; del: number } | null;
  commandCount: number;
  /** "1 think, 2 read, 3 edit" — closed-fold tally over the turn's events. */
  tally: string;
};

/** Collapsed delivery headline. Files take priority over command-only turns. */
export function formatDeliveryHeadline(delivery: Pick<TurnDelivery, "files" | "commandCount">): string {
  if (delivery.files.length > 0) {
    return t("delivery.filesChanged", { count: delivery.files.length });
  }
  if (delivery.commandCount > 0) {
    return t("delivery.commandsRun", { count: delivery.commandCount });
  }
  return t("delivery.noChanges");
}

export type TurnDeliveryUnit = {
  kind: "delivery";
  /** Stable across streaming appends: the opening message's anchor. */
  key: string;
  /**
   * Insertion point in the visibleEvents index space (same space as
   * renderUnits): the index of the next turn boundary, or MAX_SAFE_INTEGER
   * for the final turn.
   */
  anchorIndex: number;
  delivery: TurnDelivery;
};

function isTurnBoundary(event: SessionEvent): boolean {
  return event.type === "user_message" || event.type === "handoff_prepared";
}

/** cwd-relative display path; falls back to the original when not under cwd. */
function relativeToCwd(path: string, cwd: string): string {
  if (!cwd) return path;
  const pathFwd = path.replace(/\\/g, "/");
  const cwdFwd = cwd.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  if (!cwdFwd) return path;
  if (`${pathFwd.toLowerCase()}/`.startsWith(`${cwdFwd}/`)) {
    return pathFwd.slice(cwdFwd.length + 1);
  }
  return path;
}

type FileAccumulator = TurnDeliveryFile & {
  order: number;
  /** git's own line counts for the turn window (wins over tool-reported). */
  gitAdd: number | null;
  gitDel: number | null;
};

/** `C:/x/repo/a.ts` and `a.ts` (cwd = `C:/x/repo`) are the same file. */
function isAbsolutePath(path: string): boolean {
  return /^([a-zA-Z]:[\\/]|[\\/]{1,2})/.test(path);
}

export function buildTurnDeliveries(
  events: SessionEvent[],
  sessionId: string,
  cwd: string,
  isRunning: boolean,
): TurnDeliveryUnit[] {
  // renderUnits indexes live in the file_change-filtered space (visibleEvents).
  // Map every raw position to the index it occupies there so delivery anchors
  // line up with cluster/row units.
  const visibleIndexAt: number[] = new Array(events.length);
  let visibleCounter = 0;
  for (let i = 0; i < events.length; i += 1) {
    visibleIndexAt[i] = visibleCounter;
    if (events[i]!.type !== "file_change") visibleCounter += 1;
  }

  // Turn slices in the raw array: user_message / handoff_prepared open a turn.
  const boundaries: number[] = [];
  for (let i = 0; i < events.length; i += 1) {
    const event = events[i]!;
    if (event.sessionId !== sessionId) continue;
    if (isTurnBoundary(event)) boundaries.push(i);
  }

  const units: TurnDeliveryUnit[] = [];
  for (let b = 0; b < boundaries.length; b += 1) {
    const start = boundaries[b]!;
    const nextBoundary = boundaries[b + 1];
    // A queued follow-up only enters the transcript at flush time (after the
    // live turn completes), so a next boundary always implies this turn ended.
    // For the final slice, trust the seal stamped at turn/complete.
    let complete = nextBoundary !== undefined;
    if (!complete && !isRunning) complete = true;
    if (!complete) {
      let sealed = false;
      for (let i = start; i < events.length; i += 1) {
        const event = events[i]!;
        if (event.sessionId !== sessionId) continue;
        if (event.type === "assistant_message" && event.durationMs != null) {
          sealed = true;
          break;
        }
      }
      complete = sealed;
    }
    if (!complete) continue;

    const endExclusive = nextBoundary ?? events.length;
    const files = new Map<string, FileAccumulator>();
    let commandCount = 0;
    const tallyItems: { event: SessionEvent; index: number }[] = [];
    let order = 0;
    const fileAt = (path: string): FileAccumulator => {
      // One row per file: git reports workspace-relative paths (`src/a.ts`),
      // while a tool call carries whatever the agent sent — often an absolute
      // Windows path. Keying on the raw string listed every tool-edited file
      // twice (2026-10-23).
      const key = normalizeWorkspacePath(path, cwd);
      let entry = files.get(key);
      if (!entry) {
        entry = {
          path,
          relPath: relativeToCwd(path, cwd),
          add: null,
          del: null,
          touched: 0,
          order: order++,
          gitAdd: null,
          gitDel: null,
        };
        files.set(key, entry);
      } else if (!isAbsolutePath(path) && isAbsolutePath(entry.path)) {
        // Prefer the relative form for the tooltip, the diff request and undo.
        entry.path = path;
        entry.relPath = relativeToCwd(path, cwd);
      }
      return entry;
    };

    for (let i = start; i < endExclusive; i += 1) {
      const event = events[i]!;
      if (event.sessionId !== sessionId) continue;
      if (event.type === "tool_call") {
        tallyItems.push({ event, index: i });
        const kind = classifyToolCall(event);
        if (kind === "command") commandCount += 1;
        if (kind === "edit" || kind === "write") {
          const path = event.path?.trim();
          if (path) {
            const entry = fileAt(path);
            entry.touched += 1;
            const stats = extractDiffStats(event.detail);
            if (stats) {
              entry.add = (entry.add ?? 0) + stats.add;
              entry.del = (entry.del ?? 0) + stats.del;
            }
          }
        }
      } else if (event.type === "thought") {
        tallyItems.push({ event, index: i });
      } else if (event.type === "file_change") {
        // Git truth: covers files written by subagents and commands too.
        // Same file changed twice in a turn → last changeType wins.
        const entry = fileAt(event.path);
        entry.changeType = event.changeType;
        if (typeof event.add === "number" && typeof event.del === "number") {
          entry.gitAdd = event.add;
          entry.gitDel = event.del;
        }
      }
    }

    // No files touched and nothing executed → no delivery card.
    if (files.size === 0 && commandCount === 0) continue;

    const orderedFiles = [...files.values()]
      .sort((a, z) => a.order - z.order)
      .map(({ order: _order, gitAdd, gitDel, ...file }) => ({
        ...file,
        add: gitAdd ?? file.add,
        del: gitDel ?? file.del,
      }));
    let totalAdd = 0;
    let totalDel = 0;
    let anyStats = false;
    for (const file of orderedFiles) {
      if (file.add != null && file.del != null) {
        anyStats = true;
        totalAdd += file.add;
        totalDel += file.del;
      }
    }

    const opener = events[start]!;
    const openerAnchor =
      opener.type === "user_message" ? opener.messageId ?? opener.createdAt : opener.createdAt;
    const anchorSource = nextBoundary !== undefined ? nextBoundary : -1;
    units.push({
      kind: "delivery",
      key: `delivery-${openerAnchor}-${start}`,
      anchorIndex: anchorSource >= 0 ? visibleIndexAt[anchorSource]! : Number.MAX_SAFE_INTEGER,
      delivery: {
        files: orderedFiles,
        totalDiff: anyStats ? { add: totalAdd, del: totalDel } : null,
        commandCount,
        tally: formatActivityTally(tallyItems),
      },
    });
  }
  return units;
}
