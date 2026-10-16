import type { Session } from "./types";

/**
 * Recency in ms. Rows store either epoch-millis strings or ISO timestamps
 * depending on how old they are, so accept both and fall back to the id.
 */
export function sessionRecency(session: Session): number {
  const raw = session.lastActiveAt || session.startedAt || "";
  const asNum = Number(raw);
  if (Number.isFinite(asNum) && asNum > 1e11) return asNum;
  const parsed = Date.parse(raw);
  if (Number.isFinite(parsed)) return parsed;
  // Fallback: session-… millis ids
  const idNum = Number(String(session.id).replace(/^session-/, ""));
  return Number.isFinite(idNum) ? idNum : 0;
}

/** Pin timestamp in ms, or null when the dialog is not pinned. */
export function pinStamp(session: Session): number | null {
  const raw = session.pinnedAt;
  if (!raw) return null;
  const asNum = Number(raw);
  if (Number.isFinite(asNum) && asNum > 1e11) return asNum;
  const parsed = Date.parse(raw);
  return Number.isFinite(parsed) ? parsed : 0;
}

/**
 * Pinned dialogs first (stable order by pin time, newest pin on top), then
 * newest activity. Selecting a dialog must NOT reorder — only send/create bumps
 * recency — and pinning must not either, so the pinned group sorts on `pinnedAt`
 * rather than on activity.
 */
export function sortSessionsNewestFirst(list: Session[]): Session[] {
  return [...list].sort((a, b) => {
    const aPin = pinStamp(a);
    const bPin = pinStamp(b);
    if ((aPin != null) !== (bPin != null)) return aPin != null ? -1 : 1;
    if (aPin != null && bPin != null && aPin !== bPin) return bPin - aPin;
    return sessionRecency(b) - sessionRecency(a);
  });
}

/**
 * Rows a collapsed shelf shows: every pinned dialog, plus ids in `keepIds`
 * (unread completed turns), plus the newest remaining ones up to `preview`.
 * Pinning and "just finished" both exist to keep a dialog in reach, so those
 * rows must never end up behind "Show more".
 */
export function collapsedShelfSessions(
  sorted: Session[],
  preview: number,
  keepIds?: ReadonlySet<string>,
): Session[] {
  if (sorted.length <= preview) return sorted;
  const keep = (session: Session) =>
    pinStamp(session) != null || (keepIds?.has(session.id) ?? false);
  const sticky = sorted.filter(keep);
  const rest = sorted.filter((session) => !keep(session));
  const chosen = new Set(
    [...sticky, ...rest.slice(0, Math.max(0, preview - sticky.length))].map(
      (session) => session.id,
    ),
  );
  // Keep the caller's order (pins already sit first; unread-complete stays put).
  return sorted.filter((session) => chosen.has(session.id));
}
