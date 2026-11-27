// Presentation helpers for the diff viewer. Nothing here rewrites the patch:
// the same parsed lines are just classified for display (whitespace-only
// add/del pairs) and searched. DiffPage and UnifiedDiffView both use these so
// the find counter and the highlighted rows can never disagree.

import { parseUnifiedDiff, type DiffLine } from "./annotations";

/** Same row ceiling as the transcript diff card (audit P1-A). */
export const DIFF_MAX_RENDER_LINES = 1000;

export type DiffDisplayLine = {
  line: DiffLine;
  /** Index in the untruncated parse, for stable keys. */
  rawIndex: number;
  /** Collapsed hunk gap marker (only with `elideUnchanged`). */
  elide: boolean;
  /** The truncation announcement sits after this row. */
  showTruncation: boolean;
  /** add/del pair whose only difference is whitespace. */
  wsIgnored: boolean;
};

export type DiffDisplay = {
  lines: DiffDisplayLine[];
  elidedMiddle: number;
};

export type DiffMatch = {
  /** Row in `DiffDisplay.lines`. */
  row: number;
  /** Offsets into the line text without its +/-/space marker. */
  start: number;
  end: number;
};

/** Whitespace-insensitive comparison key for one diff line. */
export function squashDiffText(raw: string): string {
  return raw.replace(/^[-+ ]/, "").replace(/\s+/g, "");
}

/**
 * Mark add/del pairs in the same contiguous change block whose normalized
 * text is identical. They are still rendered (and still commentable) -- only
 * their colour is softened, so "ignore whitespace" never hides a line the
 * reviewer could act on.
 */
function whitespaceOnlyRows(lines: DiffLine[]): Set<number> {
  const ignored = new Set<number>();
  let i = 0;
  while (i < lines.length) {
    const current = lines[i];
    if (!current || current.type !== "del") {
      i += 1;
      continue;
    }
    let delEnd = i;
    while (delEnd < lines.length && lines[delEnd]?.type === "del") delEnd += 1;
    let addEnd = delEnd;
    while (addEnd < lines.length && lines[addEnd]?.type === "add") addEnd += 1;
    const dels = lines.slice(i, delEnd);
    const adds = lines.slice(delEnd, addEnd);
    const pairs = Math.min(dels.length, adds.length);
    for (let k = 0; k < pairs; k += 1) {
      const removed = dels[k];
      const added = adds[k];
      if (
        removed &&
        added &&
        squashDiffText(removed.raw) === squashDiffText(added.raw)
      ) {
        ignored.add(i + k);
        ignored.add(delEnd + k);
      }
    }
    i = addEnd > i ? addEnd : i + 1;
  }
  return ignored;
}

/**
 * Build the rows UnifiedDiffView actually paints: head/tail kept when a diff
 * is oversized, hunk gaps elided on request, and whitespace-only pairs marked.
 */
export function buildDiffDisplay(
  text: string,
  options: { ignoreWhitespace?: boolean; elideUnchanged?: boolean } = {},
): DiffDisplay {
  if (!text) return { lines: [], elidedMiddle: 0 };
  const all = parseUnifiedDiff(text);
  let kept = all;
  let elidedMiddle = 0;
  let head = all.length;
  if (all.length > DIFF_MAX_RENDER_LINES) {
    head = Math.floor(DIFF_MAX_RENDER_LINES * 0.7);
    const tail = DIFF_MAX_RENDER_LINES - head;
    kept = [...all.slice(0, head), ...all.slice(all.length - tail)];
    elidedMiddle = all.length - DIFF_MAX_RENDER_LINES;
  }
  const ws = options.ignoreWhitespace ? whitespaceOnlyRows(kept) : new Set<number>();
  let seenHunk = false;
  const lines = kept.map((line, index) => {
    const elide = Boolean(options.elideUnchanged && line.type === "hunk" && seenHunk);
    if (line.type === "hunk") seenHunk = true;
    return {
      line,
      rawIndex: index,
      elide,
      showTruncation: elidedMiddle > 0 && index === head,
      wsIgnored: ws.has(index),
    };
  });
  return { lines, elidedMiddle };
}

/** The searchable body of a diff line: its text without the marker. */
export function diffLineText(line: DiffLine): string {
  return line.raw.replace(/^[-+ ]/, "");
}

/**
 * Case-insensitive matches over the rendered, loaded rows. Hunk headers and
 * file metadata are skipped; matches never span two lines.
 */
export function findDiffMatches(lines: DiffDisplayLine[], query: string): DiffMatch[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const matches: DiffMatch[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const row = lines[index]!;
    if (row.elide) continue;
    if (row.line.type === "hunk" || row.line.type === "meta") continue;
    const hay = diffLineText(row.line).toLowerCase();
    let from = 0;
    while (from <= hay.length) {
      const at = hay.indexOf(needle, from);
      if (at < 0) break;
      matches.push({ row: index, start: at, end: at + needle.length });
      if (matches.length >= 5000) return matches;
      from = at + Math.max(1, needle.length);
    }
  }
  return matches;
}
