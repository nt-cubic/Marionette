import { useSyncExternalStore } from "react";

/**
 * Lazy remark-math + rehype-katex (plus the katex stylesheet).
 *
 * The scan in richMarkdown.ts decides when math is present; the first
 * MarkdownBody that sees it starts this loader, and every mounted instance
 * re-renders when the plugins arrive (useSyncExternalStore on a version
 * counter — the snapshot must be a primitive for identity checks).
 *
 * Until the chunk arrives, text renders as plain markdown: an unclosed `$$`
 * stays literal anyway, so the swap is seamless once math becomes parseable.
 */

type MathPack = {
  remarkMath: typeof import("remark-math").default;
  rehypeKatex: typeof import("rehype-katex").default;
};

let pack: MathPack | null = null;
let version = 0;
const listeners = new Set<() => void>();
let loading: Promise<void> | null = null;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

async function loadMathPack(): Promise<MathPack> {
  const [remarkMathModule, rehypeKatexModule] = await Promise.all([
    import("remark-math"),
    import("rehype-katex"),
  ]);
  try {
    await import("katex/dist/katex.min.css");
  } catch {
    // The stylesheet is cosmetic; math markup still renders without it.
  }
  return {
    remarkMath: remarkMathModule.default,
    rehypeKatex: rehypeKatexModule.default,
  };
}

/**
 * `active` = the text scanned positive for math. Returns the plugins when
 * loaded, null until then (and the caller renders without math that pass).
 */
export function useMathPack(active: boolean): MathPack | null {
  useSyncExternalStore(subscribe, () => version, () => 0);
  if (active && !pack && !loading) {
    loading = loadMathPack()
      .then((loaded) => {
        pack = loaded;
        version += 1;
        for (const notify of listeners) notify();
      })
      .catch(() => {
        // Drop the in-flight promise so a later mount can retry.
        loading = null;
      });
  }
  return active ? pack : null;
}
