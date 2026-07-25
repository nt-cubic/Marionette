/**
 * Find file paths and URLs inside agent text so cards can make them clickable.
 *
 * Bias: miss rather than over-match. A false positive turns ordinary prose into
 * a button, which is worse than a path the user has to copy by hand. Nothing
 * here touches the filesystem — resolution happens in Rust when the user acts.
 */

export type LinkTargetKind = "url" | "path";

export type LinkTarget = {
  kind: LinkTargetKind;
  /** Exact substring, ready to hand to the Rust resolver. */
  raw: string;
  start: number;
  end: number;
};

export type TextSegment =
  | { type: "text"; text: string }
  | { type: "link"; text: string; target: LinkTarget };

const URL_RE = /\bhttps?:\/\/[^\s<>()[\]{}"'`，。；：！？]+/g;

/** `D:\a\b.ts`, `\\?\D:\a`, `C:/a/b` — drive letter is what makes it a path. */
const WINDOWS_PATH_RE = /(?:\\\\\?\\)?\b[A-Za-z]:[\\/][^\s"'<>|?*，。；：！？]+/g;

/** `/usr/local/bin/thing` — needs at least two segments to beat "and/or". */
const POSIX_PATH_RE = /(?:^|[\s"'(<[])(\/(?:[\w.@+-]+\/)+[\w.@+-]+(?::\d+(?::\d+)?)?)/g;

/**
 * `src/app/App.tsx`, `docs/06-x.md:42` — a separator *and* a file extension.
 * Without the extension requirement this matches "and/or" and every fraction.
 */
const RELATIVE_PATH_RE =
  /(?:^|[\s"'(<[`])((?:\.{1,2}[\\/])?(?:[\w.@+-]+[\\/])+[\w.@+-]+\.[A-Za-z0-9]{1,8}(?::\d+(?::\d+)?)?)/g;

/** Trailing punctuation belongs to the sentence, not to the link. */
const TRAILING_JUNK = /[.,;:!?)\]}'"`，。；：！？、）】》]+$/;

function trimTrailing(raw: string): string {
  let value = raw;
  // Keep `:42` line refs; only strip punctuation that cannot end a path/URL.
  while (true) {
    const next = value.replace(TRAILING_JUNK, "");
    if (next === value) break;
    value = next;
  }
  return value;
}

function overlaps(found: LinkTarget[], start: number, end: number): boolean {
  return found.some((item) => start < item.end && end > item.start);
}

function collect(
  text: string,
  regex: RegExp,
  kind: LinkTargetKind,
  found: LinkTarget[],
  /** Index of the capture group holding the match (0 = whole match). */
  group = 0,
): void {
  regex.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = regex.exec(text)) !== null) {
    const captured = match[group] ?? "";
    if (!captured) continue;
    const offset = match[0].indexOf(captured);
    const start = match.index + (offset >= 0 ? offset : 0);
    const trimmed = trimTrailing(captured);
    if (!trimmed) continue;
    const end = start + trimmed.length;
    // URLs are collected first: a path regex must not bite into one.
    if (!overlaps(found, start, end)) {
      found.push({ kind, raw: trimmed, start, end });
    }
    // Zero-width guard for patterns that can match empty at the same index.
    if (regex.lastIndex === match.index) regex.lastIndex += 1;
  }
}

export function findLinkTargets(text: string): LinkTarget[] {
  if (!text || text.length > 200_000) return [];
  const found: LinkTarget[] = [];
  collect(text, URL_RE, "url", found);
  collect(text, WINDOWS_PATH_RE, "path", found);
  collect(text, POSIX_PATH_RE, "path", found, 1);
  collect(text, RELATIVE_PATH_RE, "path", found, 1);
  return found.sort((a, b) => a.start - b.start);
}

/** Split text into plain runs and link runs, ready to render. */
export function splitLinkSegments(text: string): TextSegment[] {
  const targets = findLinkTargets(text);
  if (targets.length === 0) return [{ type: "text", text }];

  const segments: TextSegment[] = [];
  let cursor = 0;
  for (const target of targets) {
    if (target.start > cursor) {
      segments.push({ type: "text", text: text.slice(cursor, target.start) });
    }
    segments.push({ type: "link", text: target.raw, target });
    cursor = target.end;
  }
  if (cursor < text.length) {
    segments.push({ type: "text", text: text.slice(cursor) });
  }
  return segments;
}

/** Short label for a path link — full value stays in the tooltip. */
export function linkLabel(target: LinkTarget, max = 64): string {
  const raw = target.raw;
  if (raw.length <= max) return raw;
  if (target.kind === "path") {
    const parts = raw.split(/[\\/]/);
    const tail = parts.slice(-2).join("/");
    return tail.length + 2 <= max ? `…/${tail}` : `…${raw.slice(raw.length - max + 1)}`;
  }
  return `${raw.slice(0, max - 1)}…`;
}
