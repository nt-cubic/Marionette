import type { SessionEvent } from "./types";
import { classifyToolCall, extractDiffStats, formatActivityTally } from "./turnActivity";

/**
 * Per-turn delivery summary for the end-of-turn card: which files the turn
 * touched (git truth + tool edits union), how many commands ran, and the
 * activity tally. Pure data — derived on every render, never persisted.
 */

export type TurnDeliveryChangeType = "added" | "modified" | "deleted" | "untracked";

export type TurnDeliveryFile = {
  /** Original path as the tool / git reported it. */
  path: string;
  /** Path relative to the session cwd; full path when it is not under cwd. */
  relPath: string;
  /** From turn-end git truth (`file_change`); absent in non-git projects. */
  changeType?: TurnDeliveryChangeType;
  /** Summed diff stats from this turn's edit/write tool calls; null = unknown. */
  add: number | null;
  del: number | null;
  /** How many edit/write tool calls touched this file in the turn. */
  touched: number;
};

export type TurnDelivery = {
  files: TurnDeliveryFile[];
  totalDiff: { add: number; del: number } | null;
  commandCount: number;
  /** "1 think, 2 read, 3 edit" — closed-fold tally over the turn's events. */
  tally: string;
};

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

type FileAccumulator = TurnDeliveryFile & { order: number };

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
    if (events[i].type !== "file_change") visibleCounter += 1;
  }

  // Turn slices in the raw array: user_message / handoff_prepared open a turn.
  const boundaries: number[] = [];
  for (let i = 0; i < events.length; i += 1) {
    const event = events[i];
    if (event.sessionId !== sessionId) continue;
    if (isTurnBoundary(event)) boundaries.push(i);
  }

  const units: TurnDeliveryUnit[] = [];
  for (let b = 0; b < boundaries.length; b += 1) {
    const start = boundaries[b];
    const nextBoundary = boundaries[b + 1];
    // A queued follow-up only enters the transcript at flush time (after the
    // live turn completes), so a next boundary always implies this turn ended.
    // For the final slice, trust the seal stamped at turn/complete.
    let complete = nextBoundary !== undefined;
    if (!complete && !isRunning) complete = true;
    if (!complete) {
      let sealed = false;
      for (let i = start; i < events.length; i += 1) {
        const event = events[i];
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
      let entry = files.get(path);
      if (!entry) {
        entry = {
          path,
          relPath: relativeToCwd(path, cwd),
          add: null,
          del: null,
          touched: 0,
          order: order++,
        };
        files.set(path, entry);
      }
      return entry;
    };

    for (let i = start; i < endExclusive; i += 1) {
      const event = events[i];
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
        fileAt(event.path).changeType = event.changeType;
      }
    }

    // No files touched and nothing executed → no delivery card.
    if (files.size === 0 && commandCount === 0) continue;

    const orderedFiles = [...files.values()]
      .sort((a, z) => a.order - z.order)
      .map(({ order: _order, ...file }) => file);
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

    const opener = events[start];
    const openerAnchor =
      opener.type === "user_message" ? opener.messageId ?? opener.createdAt : opener.createdAt;
    const anchorSource = nextBoundary !== undefined ? nextBoundary : -1;
    units.push({
      kind: "delivery",
      key: `delivery-${openerAnchor}-${start}`,
      anchorIndex: anchorSource >= 0 ? visibleIndexAt[anchorSource] : Number.MAX_SAFE_INTEGER,
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
