/**
 * String-level feature detection for the rich-markdown pipeline.
 *
 * The three heavy engines (shiki / katex / mermaid) are lazy-loaded chunks,
 * so something cheap must decide *whether* they are needed at all. That
 * decision is a pure string scan with the same fence rules the renderer uses:
 *
 * - anything inside a fenced block is code, never prose (bash `$$` must not
 *   load the math engine);
 * - a fence's info string classifies it (`mermaid`, `spec`);
 * - math hint delimiters (`$$`, `\[`) outside fences load katex. Single `$`
 *   is deliberately NOT a hint — the renderer runs with
 *   `singleDollarTextMath: false`, so dollar amounts in prose stay literal.
 *
 * No imports from React: scripts/verify-rich-markdown.mts runs this directly.
 */

export type RichFeatures = {
  /** At least one fenced code block → shiki is worth loading. */
  code: boolean;
  /** Math delimiters outside fences → remark-math + rehype-katex. */
  math: boolean;
  /** A ```mermaid fence → mermaid. */
  mermaid: boolean;
  /** A ```spec fence → generative-UI card renderer. */
  spec: boolean;
};

type FenceState = { marker: string | null; length: number };

function updateFenceState(line: string, state: FenceState): void {
  const match = /^\s{0,3}(`{3,}|~{3,})/.exec(line);
  if (!match) return;
  const marker = match[1]!;
  if (!state.marker) {
    state.marker = marker[0]!;
    state.length = marker.length;
    return;
  }
  if (state.marker === marker[0] && marker.length >= state.length) {
    state.marker = null;
    state.length = 0;
  }
}

const FENCE_OPEN_RE = /^\s{0,3}(`{3,}|~{3,})(.*)$/;

/** `$$` or `\[` outside fences (both are display-math delimiters). */
const MATH_HINT_RE = /\$\$|\\\[/;

export function scanRichFeatures(text: string): RichFeatures {
  const features: RichFeatures = { code: false, math: false, mermaid: false, spec: false };
  if (!text) return features;
  const fence: FenceState = { marker: null, length: 0 };
  for (const rawLine of text.replace(/\r\n?/g, "\n").split("\n")) {
    const inFence = fence.marker != null;
    if (!inFence) {
      const open = FENCE_OPEN_RE.exec(rawLine);
      if (open) {
        features.code = true;
        const info = open[2]!.trim().toLowerCase();
        if (info === "mermaid") features.mermaid = true;
        else if (info === "spec") features.spec = true;
      } else if (!features.math && MATH_HINT_RE.test(rawLine)) {
        features.math = true;
      }
    }
    updateFenceState(rawLine, fence);
  }
  return features;
}

/**
 * Run a transform on prose outside fenced code/tilde blocks only. Mirrors the
 * private helper in markdownText.ts; kept local so this module stays standalone.
 */
function mapOutsideFences(text: string, mapOutside: (segment: string) => string): string {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const fence: FenceState = { marker: null, length: 0 };
  const parts: string[] = [];
  let outside = "";

  const flushOutside = () => {
    if (outside.length === 0) return;
    parts.push(mapOutside(outside));
    outside = "";
  };

  for (const line of lines) {
    const wasInFence = fence.marker != null;
    updateFenceState(line, fence);
    const nowInFence = fence.marker != null;

    if (wasInFence || nowInFence) {
      flushOutside();
      parts.push(line);
    } else {
      outside = outside.length > 0 ? `${outside}\n${line}` : line;
    }
  }
  flushOutside();
  return parts.join("\n");
}

/** `\[ … \]` (LaTeX display math) → `$$ … $$`, the delimiter remark-math reads. */
const DISPLAY_MATH_DELIMS = /\\\[([\s\S]+?)\\\]/g;

/**
 * Rewrite `\[ … \]` display math to `$$ … $$` outside fenced code.
 *
 * Models trained on LaTeX emit `\[` often; remark-math only reads dollars.
 * Inside fences the sequence stays untouched (regex examples, escaped
 * brackets in code). `\( … \)` inline math is intentionally left alone: with
 * single-dollar math disabled there is no inline delimiter to map it to.
 */
export function normalizeMathDelimiters(text: string): string {
  if (!text.includes("\\[")) return text;
  return mapOutsideFences(text, (segment) =>
    segment.replace(DISPLAY_MATH_DELIMS, (_all, body: string) => `$$${body}$$`),
  );
}
