import { useEffect, useRef, useState } from "react";
import { collectSeenImages, type SeenImage } from "./seenImages";
import type { SessionEvent } from "./types";

const MIN_GAP_MS = 1500;

function same(a: readonly SeenImage[], b: readonly SeenImage[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i]!;
    const y = b[i]!;
    if (x.key !== y.key || x.at !== y.at || x.uses !== y.uses || x.via !== y.via) return false;
  }
  return true;
}

/**
 * The images this conversation has shown the agent, folded from the live events.
 *
 * Streaming bumps `events` on every chunk and a long transcript has hundreds of tool inputs
 * to parse, so the fold runs at most once per MIN_GAP_MS (leading + trailing) and the result
 * is only replaced when something actually changed. Switching conversation folds at once.
 */
export function useSeenImages(
  events: readonly SessionEvent[],
  cwd: string | null | undefined,
  sessionId: string,
): SeenImage[] {
  const [images, setImages] = useState<SeenImage[]>(() => collectSeenImages(events, cwd));
  const latest = useRef({ events, cwd });
  latest.current = { events, cwd };
  const lastRun = useRef(0);
  const timer = useRef<number | null>(null);
  const sessionRef = useRef(sessionId);

  useEffect(() => {
    const run = () => {
      timer.current = null;
      lastRun.current = Date.now();
      const next = collectSeenImages(latest.current.events, latest.current.cwd);
      setImages((prev) => (same(prev, next) ? prev : next));
    };
    if (sessionRef.current !== sessionId) {
      sessionRef.current = sessionId;
      if (timer.current != null) window.clearTimeout(timer.current);
      run();
      return;
    }
    if (timer.current != null) return; // a trailing run is already scheduled and will read the latest events
    const wait = MIN_GAP_MS - (Date.now() - lastRun.current);
    if (wait <= 0) run();
    else timer.current = window.setTimeout(run, wait);
  }, [events, cwd, sessionId]);

  useEffect(
    () => () => {
      if (timer.current != null) window.clearTimeout(timer.current);
    },
    [],
  );

  return images;
}
