import { useEffect, useMemo, useRef } from "react";
import type { ReactNode } from "react";
import { type DiffLine } from "../lib/annotations";
import {
  buildDiffDisplay,
  diffLineText,
  findDiffMatches,
  type DiffMatch,
} from "../lib/diffPresentation";
import { useT } from "../lib/i18n";

export type DiffLineComment = {
  side: "old" | "new";
  lineNumber: number;
  /** Line text without its +/-/space prefix. */
  quoted: string;
};

type UnifiedDiffViewProps = {
  text: string;
  className?: string;
  /** Click a line -> pin an annotation for the next send. */
  onLineComment?: (comment: DiffLineComment) => void;
  /**
   * Elide the gap between hunks with a gap marker (Codex-style) instead of
   * showing nothing. Only matters for multi-hunk diffs.
   */
  elideUnchanged?: boolean;
  /**
   * Presentation only: soften add/del pairs that differ solely in whitespace.
   * Both lines stay rendered and commentable; the patch text is untouched.
   */
  ignoreWhitespace?: boolean;
  /** Presentation only: highlight occurrences in the rendered lines. */
  findQuery?: string;
  /** The active match from `findDiffMatches`; gets the "current" marker. */
  activeMatch?: DiffMatch | null;
};

function lineClass(line: DiffLine, wsIgnored: boolean): string {
  const base = "file-diff-card__line";
  const withKind =
    line.type === "add"
      ? `${base} is-add`
      : line.type === "del"
        ? `${base} is-del`
        : line.type === "hunk"
          ? `${base} is-hunk`
          : line.type === "meta"
            ? `${base} is-meta`
            : base;
  return wsIgnored ? `${withKind} is-ws-ignored` : withKind;
}

/** Which side a line's number belongs to (context counts as the new file). */
function lineSide(line: DiffLine): "old" | "new" | null {
  if (line.type === "add") return "new";
  if (line.type === "del") return "old";
  if (line.type === "ctx") return "new";
  return null;
}

/** Paint the matches of one rendered line; the active one is marked. */
function renderLineBody(
  content: string,
  hits: DiffMatch[],
  row: number,
  active: DiffMatch | null | undefined,
): ReactNode {
  if (hits.length === 0) return content || " ";
  const nodes: ReactNode[] = [];
  let cursor = 0;
  hits.forEach((hit, index) => {
    if (hit.start > cursor) nodes.push(content.slice(cursor, hit.start));
    const current =
      active != null &&
      active.row === row &&
      active.start === hit.start &&
      active.end === hit.end;
    nodes.push(
      <mark
        key={`${hit.start}-${index}`}
        className={`file-diff-card__hit${current ? " is-current" : ""}`}
      >
        {content.slice(hit.start, hit.end)}
      </mark>,
    );
    cursor = hit.end;
  });
  if (cursor < content.length) nodes.push(content.slice(cursor));
  return nodes;
}

/** Shared line-numbered diff renderer for tool cards, previews and dialogs. */
export function UnifiedDiffView({
  text,
  className,
  onLineComment,
  elideUnchanged = false,
  ignoreWhitespace = false,
  findQuery = "",
  activeMatch = null,
}: UnifiedDiffViewProps) {
  const t = useT();
  const preRef = useRef<HTMLPreElement>(null);
  const display = useMemo(
    () => buildDiffDisplay(text, { ignoreWhitespace, elideUnchanged }),
    [text, ignoreWhitespace, elideUnchanged],
  );
  const rows = display.lines;
  const matchesByRow = useMemo(() => {
    const map = new Map<number, DiffMatch[]>();
    if (!findQuery.trim()) return map;
    for (const match of findDiffMatches(rows, findQuery)) {
      const list = map.get(match.row);
      if (list) list.push(match);
      else map.set(match.row, [match]);
    }
    return map;
  }, [findQuery, rows]);

  // Keep the current match in view. This is a local scrollIntoView on the
  // marker, so it works whether the diff sits in the page or a tool card.
  useEffect(() => {
    if (!activeMatch) return;
    const el = preRef.current?.querySelector(".file-diff-card__hit.is-current");
    el?.scrollIntoView({ block: "center" });
  }, [activeMatch?.row, activeMatch?.start]);

  return (
    <pre ref={preRef} className={className}>
      {rows.map(({ line, elide, showTruncation, wsIgnored }, index) => {
        const side = lineSide(line);
        const lineNumber =
          side === "new" ? line.newLine : side === "old" ? line.oldLine : null;
        const canComment =
          Boolean(onLineComment) && side != null && lineNumber != null && line.type !== "meta";
        const isCodeLine =
          line.type === "add" || line.type === "del" || line.type === "ctx";
        const hits = matchesByRow.get(index) ?? [];
        const marker =
          isCodeLine && /^[-+ ]/.test(line.raw) ? line.raw.slice(0, 1) : "";
        const body =
          findQuery.trim() && isCodeLine ? (
            <>
              {marker}
              {renderLineBody(diffLineText(line), hits, index, activeMatch)}
            </>
          ) : (
            line.raw || " "
          );

        return (
          <span key={index}>
            {(elide || showTruncation) && (
              <span className="file-diff-card__line is-elide" aria-hidden>
                <span className="file-diff-card__gutter">
                  <span className="file-diff-card__ln" />
                  <span className="file-diff-card__ln" />
                </span>
                <span className="file-diff-card__code">
                  {showTruncation
                    ? t("md.elidedMiddleLines", { n: display.elidedMiddle })
                    : "\u22EF"}
                </span>
              </span>
            )}
            <span className={lineClass(line, wsIgnored)}>
              <span className="file-diff-card__gutter" aria-hidden>
                <span className="file-diff-card__ln file-diff-card__ln--old">
                  {line.oldLine ?? ""}
                </span>
                <span className="file-diff-card__ln file-diff-card__ln--new">
                  {line.newLine ?? ""}
                </span>
              </span>
              {canComment ? (
                <button
                  type="button"
                  className="file-diff-card__code"
                  title={t("md.commentThisLine")}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onLineComment?.({
                      side: side!,
                      lineNumber: lineNumber!,
                      quoted: line.raw.replace(/^[-+ ]/, ""),
                    });
                  }}
                >
                  {body}
                </button>
              ) : (
                <span className="file-diff-card__code">{body}</span>
              )}
              {"\n"}
            </span>
          </span>
        );
      })}
    </pre>
  );
}
