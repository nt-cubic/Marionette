import { useMemo } from "react";
import { parseUnifiedDiff, type DiffLine } from "../lib/annotations";
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
  /** Click a line → pin an annotation for the next send. */
  onLineComment?: (comment: DiffLineComment) => void;
  /**
   * Elide the gap between hunks with a `⋮` row (Codex-style) instead of
   * showing nothing. Only matters for multi-hunk diffs.
   */
  elideUnchanged?: boolean;
};

/**
 * Hard row ceiling (audit P1-A): one oversized diff card used to mount every
 * line × 6-7 spans, so a single huge paste could add tens of thousands of DOM
 * nodes to the transcript. The card keeps its head and tail; the middle gap is
 * announced instead of rendered.
 */
const MAX_RENDER_LINES = 1000;

function lineClass(line: DiffLine): string {
  const base = "file-diff-card__line";
  if (line.type === "add") return `${base} is-add`;
  if (line.type === "del") return `${base} is-del`;
  if (line.type === "hunk") return `${base} is-hunk`;
  if (line.type === "meta") return `${base} is-meta`;
  return base;
}

/** Which side a line's number belongs to (context counts as the new file). */
function lineSide(line: DiffLine): "old" | "new" | null {
  if (line.type === "add") return "new";
  if (line.type === "del") return "old";
  if (line.type === "ctx") return "new";
  return null;
}

/** Shared line-numbered diff renderer for tool cards, previews and dialogs. */
export function UnifiedDiffView({
  text,
  className,
  onLineComment,
  elideUnchanged = false,
}: UnifiedDiffViewProps) {
  const t = useT();
  const { parsed, elidedMiddle } = useMemo(() => {
    if (!text) return { parsed: [] as DiffLine[], elidedMiddle: 0 };
    const all = parseUnifiedDiff(text);
    if (all.length <= MAX_RENDER_LINES) return { parsed: all, elidedMiddle: 0 };
    // Keep the head and tail (where the action usually is), elide the middle.
    const head = Math.floor(MAX_RENDER_LINES * 0.7);
    const tail = MAX_RENDER_LINES - head;
    return {
      parsed: [...all.slice(0, head), ...all.slice(all.length - tail)],
      elidedMiddle: all.length - MAX_RENDER_LINES,
    };
  }, [text]);
  // `⋮` marks the gap between hunks — never the header lines before the first.
  const rows = useMemo(() => {
    let seenHunk = false;
    return parsed.map((line, index) => {
      const elide = elideUnchanged && line.type === "hunk" && seenHunk;
      if (line.type === "hunk") seenHunk = true;
      // The truncation gap sits right after the head block (index === head).
      const showTruncation = elidedMiddle > 0 && index === Math.floor(MAX_RENDER_LINES * 0.7);
      return { line, elide, showTruncation };
    });
  }, [parsed, elideUnchanged, elidedMiddle]);

  return (
    <pre className={className}>
      {rows.map(({ line, elide, showTruncation }, index) => {
        const side = lineSide(line);
        const lineNumber = side === "new" ? line.newLine : side === "old" ? line.oldLine : null;
        const canComment =
          Boolean(onLineComment) && side != null && lineNumber != null && line.type !== "meta";

        return (
          <span key={index}>
            {(elide || showTruncation) && (
              <span className="file-diff-card__line is-elide" aria-hidden>
                <span className="file-diff-card__gutter">
                  <span className="file-diff-card__ln" />
                  <span className="file-diff-card__ln" />
                </span>
                <span className="file-diff-card__code">
                  {showTruncation ? t("md.elidedMiddleLines", { n: elidedMiddle }) : "⋮"}
                </span>
              </span>
            )}
            <span className={lineClass(line)}>
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
                  {line.raw || " "}
                </button>
              ) : (
                <span className="file-diff-card__code">{line.raw || " "}</span>
              )}
              {"\n"}
            </span>
          </span>
        );
      })}
    </pre>
  );
}
