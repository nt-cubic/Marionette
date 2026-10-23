import { useEffect, useState } from "react";
import { ChevronDown } from "lucide-react";
import { getFileDiff } from "../lib/api";
import type { TurnDelivery, TurnDeliveryChangeType, TurnDeliveryFile } from "../lib/turnDelivery";
import { UnifiedDiffView } from "./UnifiedDiffView";

/** Same single-letter glyphs as the right-side Changed Files panel. */
function badgeLetter(changeType: TurnDeliveryChangeType): string {
  switch (changeType) {
    case "added":
      return "A";
    case "deleted":
      return "D";
    case "untracked":
      return "U";
    default:
      return "M";
  }
}

function DeliveryFileRow({
  file,
  projectId,
}: {
  file: TurnDeliveryFile;
  projectId?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const [diff, setDiff] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Deleted files have no workspace diff to load; others need a project context.
  const canDiff = file.changeType !== "deleted" && Boolean(projectId);

  useEffect(() => {
    if (!expanded || diff !== null || loading) return;
    if (!projectId || !file.path) {
      setError("No project context for diff");
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    void getFileDiff(projectId, file.path)
      .then((text) => {
        if (!cancelled) setDiff(text || "(empty diff)");
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [expanded, projectId, file.path, diff, loading]);

  const title =
    file.touched > 1 ? `${file.path} · 本轮编辑 ${file.touched} 次` : file.path;

  return (
    <>
      <button
        type="button"
        className={`turn-delivery__row${canDiff ? " is-expandable" : ""}`}
        title={title}
        onClick={canDiff ? () => setExpanded((value) => !value) : undefined}
        aria-expanded={canDiff ? expanded : undefined}
      >
        {file.changeType ? (
          <span className={`change-badge change-${file.changeType}`}>
            {badgeLetter(file.changeType)}
          </span>
        ) : (
          <span className="turn-delivery__dot" aria-hidden />
        )}
        <span className="turn-delivery__path">{file.relPath}</span>
        <span className="turn-delivery__stats">
          {file.add != null && file.add > 0 && (
            <span className="turn-delivery__stat turn-delivery__stat--add">+{file.add}</span>
          )}
          {file.del != null && file.del > 0 && (
            <span className="turn-delivery__stat turn-delivery__stat--del">−{file.del}</span>
          )}
        </span>
        {canDiff && (
          <ChevronDown
            size={12}
            className={`turn-delivery__chevron${expanded ? " is-open" : ""}`}
            aria-hidden
          />
        )}
      </button>
      {expanded && (
        <div className="turn-delivery__diff">
          {loading && <div className="turn-delivery__diff-note">Loading diff…</div>}
          {error && <div className="turn-delivery__diff-note">{error}</div>}
          {diff !== null && (
            <UnifiedDiffView
              className="file-diff-card__body custom-scrollbar scrollbar-autohide"
              text={diff}
              elideUnchanged
            />
          )}
        </div>
      )}
    </>
  );
}

/**
 * End-of-turn delivery card: what this turn changed. Files are the git-truth +
 * tool-edit union from `buildTurnDeliveries`; each row lazily expands into the
 * workspace diff (same loader as the Changed Files panel).
 */
export function TurnDeliveryCard({
  delivery,
  projectId,
}: {
  delivery: TurnDelivery;
  projectId?: string;
}) {
  const [open, setOpen] = useState(false);

  const previewParts = [`${delivery.files.length} 个文件`];
  if (delivery.commandCount > 0) previewParts.push(`跑了 ${delivery.commandCount} 条命令`);

  return (
    <details
      className="event-card event-card--collapsible event-card--turn_delivery"
      open={open}
      onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}
    >
      <summary className="event-card__summary" title={delivery.files.map((f) => f.relPath).join("\n")}>
        <span className="event-card__type">交付</span>
        <span className="event-card__preview-wrap">
          <span className="event-card__preview">{previewParts.join(" · ")}</span>
        </span>
        {delivery.totalDiff && (
          <span className="event-card__tool-diff">
            {delivery.totalDiff.add > 0 && (
              <span className="event-card__tool-diff-add">+{delivery.totalDiff.add}</span>
            )}
            {delivery.totalDiff.del > 0 && (
              <span className="event-card__tool-diff-del">−{delivery.totalDiff.del}</span>
            )}
          </span>
        )}
      </summary>
      {open && (
        <div className="event-card__expand-slot">
          <div className="turn-delivery">
            <div className="turn-delivery__files">
              {delivery.files.map((file) => (
                <DeliveryFileRow key={file.path} file={file} projectId={projectId} />
              ))}
            </div>
            {delivery.tally && <div className="turn-delivery__tally">{delivery.tally}</div>}
          </div>
        </div>
      )}
    </details>
  );
}
