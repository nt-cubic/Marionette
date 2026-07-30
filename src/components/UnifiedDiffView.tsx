import { useMemo } from "react";
import { parseUnifiedDiff } from "../lib/annotations";

type UnifiedDiffViewProps = {
  text: string;
  className?: string;
};

/** Shared line-numbered diff renderer for workspace previews and dialogs. */
export function UnifiedDiffView({ text, className }: UnifiedDiffViewProps) {
  const parsed = useMemo(() => (text ? parseUnifiedDiff(text) : []), [text]);

  return (
    <pre className={className}>
      {parsed.map((line, index) => {
        const lineClass =
          line.type === "add"
            ? "file-diff-card__line is-add"
            : line.type === "del"
              ? "file-diff-card__line is-del"
              : line.type === "hunk"
                ? "file-diff-card__line is-hunk"
                : "file-diff-card__line";
        const lineNumberSide =
          line.type === "add"
            ? "new"
            : line.type === "del"
              ? "old"
              : line.type === "ctx"
                ? "new"
                : null;

        return (
          <span className={lineClass} key={index}>
            <span className="file-diff-card__gutter" aria-hidden>
              <span className="file-diff-card__ln file-diff-card__ln--old">
                {line.oldLine ?? ""}
              </span>
              <span className="file-diff-card__ln file-diff-card__ln--new">
                {line.newLine ?? ""}
              </span>
            </span>
            <span
              className={`file-diff-card__code${lineNumberSide ? ` is-${lineNumberSide}` : ""}`}
            >
              {line.raw || " "}
            </span>
            {"\n"}
          </span>
        );
      })}
    </pre>
  );
}
