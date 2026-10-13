/** Near-bottom threshold matching the chat list's stick-to-bottom slack. */
export const TRANSCRIPT_NEAR_BOTTOM_PX = 80;

export type TranscriptScrollSnap = {
  stickToBottom: boolean;
  scrollTop: number;
};

export function isNearTranscriptBottom(
  el: { scrollTop: number; scrollHeight: number; clientHeight: number },
  nearPx = TRANSCRIPT_NEAR_BOTTOM_PX,
): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight < nearPx;
}

export function captureScrollSnap(
  el: { scrollTop: number; scrollHeight: number; clientHeight: number },
): TranscriptScrollSnap {
  return {
    stickToBottom: isNearTranscriptBottom(el),
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
