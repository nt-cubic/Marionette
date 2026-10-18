/**
 * Per-session ACP render budget: which streams may publish into React,
 * and how to flush shadow rails when a session enters the paint set.
 *
 * Cheap path still runs the same apply+coalesce wrapper in App; this module
 * only classifies publish vs shadow-only and copies shadow → published.
 */

export type ChunkKind = "thought" | "message" | "meta";

export function paintSet(args: {
  visibleSessionIds: string[];
  unfinishedDelegateChildren: Iterable<{ childId: string; parentId: string }>;
}): Set<string> {
  const set = new Set(args.visibleSessionIds.filter(Boolean));
  for (const { childId, parentId } of args.unfinishedDelegateChildren) {
    if (childId && parentId && set.has(parentId)) set.add(childId);
  }
  return set;
}

export function shouldPublishChunk(
  sessionId: string,
  kind: ChunkKind,
  paint: ReadonlySet<string>,
): "now" | "shadow-only" {
  if (kind === "meta") return "now";
  return paint.has(sessionId) ? "now" : "shadow-only";
}

export function chunkKindFromRole(role: string): ChunkKind {
  if (role === "thought") return "thought";
  if (role === "assistant") return "message";
  return "meta";
}

/**
 * Publish every paintSet id whose shadow rail is ahead of (or missing from)
 * the published map. Parent and unfinished @-delegate children are flushed
 * together when the caller passes the full next paint set.
 *
 * Mutates `published` in place. Returns the ids that actually changed.
 */
export function flushShadowsIntoPaintSet<T>(
  nextPaintSet: ReadonlySet<string>,
  shadow: Map<string, T[]>,
  published: Map<string, T[]>,
): string[] {
  const flushed: string[] = [];
  for (const id of nextPaintSet) {
    const rail = shadow.get(id);
    if (rail === undefined) continue;
    if (published.get(id) !== rail) {
      published.set(id, rail);
      flushed.push(id);
    }
  }
  return flushed;
}

export function unfinishedDelegateChildrenFromSessions(
  sessions: Iterable<{ id: string; parentSessionId?: string | null; status?: string }>,
): Array<{ childId: string; parentId: string }> {
  const out: Array<{ childId: string; parentId: string }> = [];
  for (const s of sessions) {
    const parentId = s.parentSessionId;
    if (!parentId) continue;
    if (s.status === "exited" || s.status === "error") continue;
    out.push({ childId: s.id, parentId });
  }
  return out;
}

export function unfinishedDelegateChildrenFromMeta(
  delegateMeta: Iterable<[string, { parentId?: string; finished?: boolean }]>,
): Array<{ childId: string; parentId: string }> {
  const out: Array<{ childId: string; parentId: string }> = [];
  for (const [childId, meta] of delegateMeta) {
    if (!childId || !meta?.parentId || meta.finished) continue;
    out.push({ childId, parentId: meta.parentId });
  }
  return out;
}
