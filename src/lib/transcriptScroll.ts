/** Slack while already stuck: layout jitter / streaming growth stay pinned. */
export const TRANSCRIPT_NEAR_BOTTOM_PX = 80;
/**
 * After an explicit scroll-up, only the true end re-arms stick.
 * Must stay smaller than one mouse-wheel notch and larger than sub-pixel noise.
 */
export const TRANSCRIPT_RESTICK_PX = 8;

export type TranscriptScrollSnap = {
  stickToBottom: boolean;
  scrollTop: number;
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
): TranscriptScrollSnap {
  return {
    stickToBottom,
    scrollTop: Math.max(0, el.scrollTop),
  };
}

/**
 * First visit / last-seen-at-tail → pin to the live end.
 * Otherwise restore the pixel offset captured when leaving the tab.
 */
export function applyScrollSnap(
  el: { scrollTop: number; scrollHeight: number },
  snap: TranscriptScrollSnap | undefined,
): "bottom" | "offset" {
  if (!snap || snap.stickToBottom) {
    el.scrollTop = el.scrollHeight;
    return "bottom";
  }
  el.scrollTop = Math.max(0, snap.scrollTop);
  return "offset";
}
