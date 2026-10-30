import { useEffect, useState } from "react";
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

/**
 * Shiki highlighting for fenced code blocks (curated fine-grained bundle).
 *
 * We ship a hand-picked grammar set instead of the full `shiki` bundle
 * (317 languages, 12.2MB of dist chunks — 70% of the old frontend payload).
 * Grammars still load lazily one by one on first use, and unknown language
 * ids fall back to plain text exactly like before ("tried, not
 * highlightable" caches as null so they don't retry).
 *
 * The JS regex engine replaces oniguruma-wasm: one less 600KB chunk, and
 * `forgiving` keeps a rare incompatible pattern from throwing. It is slower
 * than wasm oniguruma, but highlighting is debounced 200ms and cached below,
 * so streaming edits never pay per-delta cost.
 *
 * The cache matters because the transcript list virtualizes: rows unmount on
 * scroll and remount later, and a remount must neither re-highlight nor
 * flash back to plain text.
 */

/** Grammar id → lazy module thunk. Ids must match shiki/langs/<id>.mjs. */
const LANG_LOADERS: Record<string, () => Promise<unknown>> = {
  bash: () => import("shiki/langs/bash.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  cpp: () => import("shiki/langs/cpp.mjs"),
  csharp: () => import("shiki/langs/csharp.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  dockerfile: () => import("shiki/langs/dockerfile.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  jsonc: () => import("shiki/langs/jsonc.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  kotlin: () => import("shiki/langs/kotlin.mjs"),
  latex: () => import("shiki/langs/latex.mjs"),
  lua: () => import("shiki/langs/lua.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  "objective-c": () => import("shiki/langs/objective-c.mjs"),
  perl: () => import("shiki/langs/perl.mjs"),
  php: () => import("shiki/langs/php.mjs"),
  powershell: () => import("shiki/langs/powershell.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  ruby: () => import("shiki/langs/ruby.mjs"),
  rust: () => import("shiki/langs/rust.mjs"),
  scala: () => import("shiki/langs/scala.mjs"),
  scss: () => import("shiki/langs/scss.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  swift: () => import("shiki/langs/swift.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  vb: () => import("shiki/langs/vb.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
};

/**
 * Extra aliases beyond each grammar's own metadata (shiki registers
 * grammar-declared aliases like sh/zsh/py automatically; these are ids the
 * grammars themselves don't cover).
 */
const LANG_ALIASES: Record<string, string> = {
  "c++": "cpp",
  "c#": "csharp",
  cs: "csharp",
  golang: "go",
  rs: "rust",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  node: "javascript",
  ts: "typescript",
  yml: "yaml",
  md: "markdown",
  py3: "python",
  ps1: "powershell",
  posh: "powershell",
  shell: "bash",
  sh: "bash",
  zsh: "bash",
  htm: "html",
  tex: "latex",
  docker: "dockerfile",
};

/** Normalize a fence info word to a shipped grammar id (null = plain). */
function resolveLangId(rawLang: string): string | null {
  let id = rawLang.trim().toLowerCase();
  if (id.startsWith("#")) id = id.slice(1);
  if (!id) return null;
  id = LANG_ALIASES[id] ?? id;
  return id in LANG_LOADERS ? id : null;
}

const cache = new Map<string, string | null>();
const CACHE_LIMIT = 120;
let coreLoader: Promise<HighlighterCore> | null = null;
/** Grammar loads in flight, so parallel blocks don't double-load one lang. */
const loading = new Map<string, Promise<void>>();

function loadCore(): Promise<HighlighterCore> {
  coreLoader ??= Promise.all([
    import("shiki/themes/github-light.mjs"),
    import("shiki/themes/github-dark-default.mjs"),
  ]).then(([light, dark]) =>
    createHighlighterCore({
      themes: [light, dark],
      langs: [],
      engine: createJavaScriptRegexEngine({ forgiving: true }),
    }),
  );
  return coreLoader;
}

async function ensureLang(core: HighlighterCore, id: string): Promise<void> {
  if (core.getLoadedLanguages().includes(id)) return;
  let pending = loading.get(id);
  if (!pending) {
    const loader = LANG_LOADERS[id];
    if (!loader) return;
    pending = loader()
      .then((mod) => core.loadLanguage(mod as never))
      .finally(() => loading.delete(id));
    loading.set(id, pending);
  }
  await pending;
}

async function highlight(code: string, id: string): Promise<string | null> {
  const core = await loadCore();
  await ensureLang(core, id);
  // Dual theme with defaultColor:false: spans carry both palettes
  // as --shiki-light/--shiki-dark and CSS picks by app theme.
  return core.codeToHtml(code, {
    lang: id,
    themes: { light: "github-light", dark: "github-dark-default" },
    defaultColor: false,
  });
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
    const id = resolveLangId(lang);
    if (!id) {
      // Unknown language id → stay plain (cached so it doesn't retry).
      cache.set(key, null);
      setHtml(null);
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      highlight(code, id)
        .then((out) => {
          if (cancelled) return;
          if (cache.size >= CACHE_LIMIT) {
            const oldest = cache.keys().next().value;
            if (oldest !== undefined) cache.delete(oldest);
          }
          cache.set(key, out);
          setHtml(out);
        })
        .catch(() => {
          // Grammar load or highlight failure → stay plain.
          if (!cancelled) {
            cache.set(key, null);
            setHtml(null);
          }
        });
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [code, key, lang]);

  return html;
}
