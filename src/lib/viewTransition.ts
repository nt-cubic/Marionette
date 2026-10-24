import { flushSync } from "react-dom";

/**
 * Run a state change that repaints the whole window (theme switch) inside a
 * View Transition, so the old and new frames cross-fade instead of snapping.
 * Falls back to a plain update when the API is missing or the user asked for
 * reduced motion.
 */
export function withViewTransition(update: () => void): void {
  const doc = document as Document & { startViewTransition?: (cb: () => void) => unknown };
  const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  if (!doc.startViewTransition || reduce) {
    update();
    return;
  }
  doc.startViewTransition(() => {
    flushSync(update);
  });
}
