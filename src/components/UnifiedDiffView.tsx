import { useMemo } from "react";
import { parseUnifiedDiff, type DiffLine } from "../lib/annotations";

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
  const parsed = useMemo(() => (text ? parseUnifiedDiff(text) : []), [text]);
  // `⋮` marks the gap between hunks — never the header lines before the first.
  const rows = useMemo(() => {
    let seenHunk = false;
    return parsed.map((line) => {
      const elide = elideUnchanged && line.type === "hunk" && seenHunk;
      if (line.type === "hunk") seenHunk = true;
      return { line, elide };
    });
  }, [parsed, elideUnchanged]);

  return (
    <pre className={className}>
      {rows.map(({ line, elide }, index) => {
        const side = lineSide(line);
        const lineNumber = side === "new" ? line.newLine : side === "old" ? line.oldLine : null;
        const canComment =
          Boolean(onLineComment) && side != null && lineNumber != null && line.type !== "meta";

        return (
          <span key={index}>
            {elide && (
              <span className="file-diff-card__line is-elide" aria-hidden>
                <span className="file-diff-card__gutter">
                  <span className="file-diff-card__ln" />
                  <span className="file-diff-card__ln" />
                </span>
                <span className="file-diff-card__code">⋮</span>
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
                  title="评论此行"
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
