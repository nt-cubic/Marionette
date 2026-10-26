import { useEffect, useState } from "react";

/**
 * Shiki highlighting for fenced code blocks.
 *
 * Shiki ships a small core plus per-language grammar chunks; `codeToHtml`
 * from the main entry lazy-loads what it needs, so this module only has to
 * own the engine chunk load and a highlight cache.
 *
 * The cache matters because the transcript list virtualizes: rows unmount on
 * scroll and remount later, and a remount must neither re-highlight nor
 * flash back to plain text. `null` is cached too ("tried, not highlightable")
 * so unsupported language ids don't retry forever.
 *
 * Highlighting is debounced 200ms: during streaming every tick changes the
 * code, and re-highlighting each delta of a long block is waste. The block
 * renders plain (exactly the pre-pipeline look) and swaps to colored in
 * place once the text goes quiet.
 */

const cache = new Map<string, string | null>();
const CACHE_LIMIT = 120;
let shikiLoader: Promise<typeof import("shiki")> | null = null;

function loadShiki(): Promise<typeof import("shiki")> {
  shikiLoader ??= import("shiki");
  return shikiLoader;
}

function cacheKey(lang: string, code: string): string {
  return `${lang}\u0000${code}`;
}

/**
 * Highlighted HTML for one code block, or null while plain should render
 * (engine loading, debounce window, or the language has no grammar).
 */
export function useShikiHtml(lang: string, code: string): string | null {
  const key = lang ? cacheKey(lang, code) : "";
  const [html, setHtml] = useState<string | null>(() =>
    key ? (cache.get(key) ?? null) : null,
  );

  useEffect(() => {
    if (!key) {
      setHtml(null);
      return;
    }
    const cached = cache.get(key);
    if (cached !== undefined) {
      setHtml(cached);
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void loadShiki()
        .then(async (shiki) => {
          let out: string | null = null;
          try {
            // Dual theme with defaultColor:false: spans carry both palettes
            // as --shiki-light/--shiki-dark and CSS picks by app theme.
            out = await shiki.codeToHtml(code, {
              lang,
              themes: { light: "github-light", dark: "github-dark-default" },
              defaultColor: false,
            });
          } catch {
            // Unknown language id or grammar load failure → stay plain.
            out = null;
          }
          if (cancelled) return;
          if (cache.size >= CACHE_LIMIT) {
            const oldest = cache.keys().next().value;
            if (oldest !== undefined) cache.delete(oldest);
          }
          cache.set(key, out);
          setHtml(out);
        })
        .catch(() => {
          if (!cancelled) setHtml(null);
        });
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [code, key, lang]);

  return html;
}
