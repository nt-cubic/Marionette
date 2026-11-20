/** Slack while already stuck: layout jitter / streaming growth stay pinned. */
export const TRANSCRIPT_NEAR_BOTTOM_PX = 80;
/**
 * After an explicit scroll-up, only the true end re-arms stick.
 * Must stay smaller than one mouse-wheel notch and larger than sub-pixel noise.
 */
export const TRANSCRIPT_RESTICK_PX = 8;

/** A visible transcript unit plus the pixel offset into that unit. */
export type TranscriptScrollAnchor = {
  index: number;
  key?: string;
  offset: number;
};

export type TranscriptAnchorRect = {
  index: number;
  key?: string;
  top: number;
};

export type TranscriptScrollSnap = {
  stickToBottom: boolean;
  scrollTop: number;
  /** Survives image/layout growth while a tab is restoring. */
  anchor?: TranscriptScrollAnchor;
};

/** Live stick + "user just wheeled up" hold-off. */
export type TranscriptStickState = {
  stickToBottom: boolean;
  released: boolean;
};

export function distanceFromTranscriptBottom(
  el: { scrollTop: number; scrollHeight: number; clientHeight: number },
): number {
  return el.scrollHeight - el.scrollTop - el.clientHeight;
}

export function isNearTranscriptBottom(
  el: { scrollTop: number; scrollHeight: number; clientHeight: number },
  nearPx = TRANSCRIPT_NEAR_BOTTOM_PX,
): boolean {
  return distanceFromTranscriptBottom(el) < nearPx;
}

/**
 * Wheel-up while stuck (or already in the hold-off) must unstick immediately,
 * before the following scroll event re-evaluates the 80px slack.
 */
export function applyTranscriptStickOnWheelUp(
  state: TranscriptStickState,
  deltaY: number,
): TranscriptStickState {
  if (deltaY < 0 && (state.stickToBottom || state.released)) {
    return { stickToBottom: false, released: true };
  }
  return state;
}

/**
 * Scroll after a wheel-up must not re-stick while still inside the 80px slack.
 * Re-arm when the user leaves that zone, or when they scroll down to the true end.
 */
export function applyTranscriptStickOnScroll(
  state: TranscriptStickState,
  distanceFromBottom: number,
  scrolledDown: boolean,
): TranscriptStickState {
  if (state.released) {
    if (distanceFromBottom <= TRANSCRIPT_RESTICK_PX && scrolledDown) {
      return { stickToBottom: true, released: false };
    }
    if (distanceFromBottom >= TRANSCRIPT_NEAR_BOTTOM_PX) {
      return { stickToBottom: false, released: false };
    }
    return { stickToBottom: false, released: true };
  }
  return {
    stickToBottom: distanceFromBottom < TRANSCRIPT_NEAR_BOTTOM_PX,
    released: false,
  };
}

export function captureScrollSnap(
  el: { scrollTop: number; scrollHeight: number; clientHeight: number },
  stickToBottom = isNearTranscriptBottom(el),
  anchor?: TranscriptScrollAnchor,
): TranscriptScrollSnap {
  return {
    stickToBottom,
    scrollTop: Math.max(0, el.scrollTop),
    ...(anchor ? { anchor } : {}),
  };
}

/** Pick the last mounted transcript unit at or above the viewport top. */
export function captureScrollAnchor(
  units: readonly TranscriptAnchorRect[],
  scrollTop: number,
): TranscriptScrollAnchor | undefined {
  if (units.length === 0) return undefined;
  let anchor = units[0]!;
  for (const unit of units) {
    if (unit.top <= scrollTop + 0.5 && unit.top >= anchor.top) anchor = unit;
  }
  return {
    index: anchor.index,
    ...(anchor.key ? { key: anchor.key } : {}),
    offset: scrollTop - anchor.top,
  };
}

/** Resolve an anchor after cards above it have loaded or changed size. */
export function resolveScrollAnchorTop(
  anchor: TranscriptScrollAnchor | undefined,
  topForIndex: (index: number) => number | undefined,
): number | undefined {
  if (!anchor) return undefined;
  const top = topForIndex(anchor.index);
  if (top == null || !Number.isFinite(top)) return undefined;
  return Math.max(0, top + anchor.offset);
}

/**
 * Session tab positions outlive the mounted SessionView. Non-session surface
 * tabs (memo/doc/browser) unmount it, so a component-local ref used to lose
 * the illustrated tab's camera before the user returned.
 */
const TRANSCRIPT_SCROLL_SNAP_LIMIT = 120;
const transcriptScrollSnaps = new Map<string, TranscriptScrollSnap>();

export function rememberTranscriptScrollSnap(
  sessionId: string,
  snap: TranscriptScrollSnap,
) {
  transcriptScrollSnaps.delete(sessionId);
  transcriptScrollSnaps.set(sessionId, snap);
  while (transcriptScrollSnaps.size > TRANSCRIPT_SCROLL_SNAP_LIMIT) {
    const oldest = transcriptScrollSnaps.keys().next().value;
    if (oldest == null) break;
    transcriptScrollSnaps.delete(oldest);
  }
}

export function readTranscriptScrollSnap(
  sessionId: string,
): TranscriptScrollSnap | undefined {
  return transcriptScrollSnaps.get(sessionId);
}

/** Test helper â not used by the app. */
export function resetTranscriptScrollSnaps() {
  transcriptScrollSnaps.clear();
}

/**
 * First visit / last-seen-at-tail → pin to the live end.
 * Otherwise restore the pixel offset captured when leaving the tab.
 */
export function applyScrollSnap(
  el: { scrollTop: number; scrollHeight: number },
  snap: TranscriptScrollSnap | undefined,
  anchorTop?: number,
): "bottom" | "offset" {
  if (!snap || snap.stickToBottom) {
    el.scrollTop = el.scrollHeight;
    return "bottom";
  }
  el.scrollTop = Math.max(0, anchorTop ?? snap.scrollTop);
  return "offset";
}
