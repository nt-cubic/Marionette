import { useEffect, useState } from "react";
import { ChevronDown, Undo2 } from "lucide-react";
import { CHAT_PROJECT_ID, getFileDiff, restoreTurnFiles } from "../lib/api";
import type { TurnDelivery, TurnDeliveryChangeType, TurnDeliveryFile } from "../lib/turnDelivery";
import { formatDeliveryHeadline } from "../lib/turnDelivery";
import { ConfirmDialog } from "./ConfirmDialog";
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

  const canDiff = Boolean(projectId) && projectId !== CHAT_PROJECT_ID && Boolean(file.path);

  useEffect(() => {
    if (!expanded) return;
    if (!projectId || projectId === CHAT_PROJECT_ID || !file.path) {
      setError("没有项目上下文，无法加载 diff");
      setLoading(false);
      return;
    }
    // Do not put `loading` / `diff` in the dep list: setting loading true would
    // re-run the effect, cancel the in-flight invoke, and freeze on "Loading…".
    let cancelled = false;
    let timeoutId: number | undefined;
    setLoading(true);
    setError(null);
    const timeout = new Promise<never>((_, reject) => {
      timeoutId = window.setTimeout(() => reject(new Error("Diff 请求超时")), 8000);
    });
    void Promise.race([getFileDiff(projectId, file.path), timeout])
      .then((text) => {
        if (!cancelled) setDiff(text.trim() ? text : "（无差异）");
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
          setDiff(null);
        }
      })
      .finally(() => {
        if (timeoutId != null) window.clearTimeout(timeoutId);
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
      if (timeoutId != null) window.clearTimeout(timeoutId);
    };
  }, [expanded, projectId, file.path]);

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
          {loading && diff === null && (
            <div className="turn-delivery__diff-note">正在加载 diff…</div>
          )}
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
  undone = false,
  onUndone,
  onRestored,
}: {
  delivery: TurnDelivery;
  projectId?: string;
  undone?: boolean;
  onUndone?: () => void;
  onRestored?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const headline = formatDeliveryHeadline(delivery);
  const fileCount = delivery.files.length;
  const canUndo =
    !undone &&
    !busy &&
    fileCount > 0 &&
    Boolean(projectId) &&
    projectId !== CHAT_PROJECT_ID;
  const title = delivery.files.map((f) => f.relPath).join("\n");

  const runUndo = async () => {
    if (!projectId || fileCount === 0) return;
    setBusy(true);
    setError(null);
    try {
      const result = await restoreTurnFiles(
        projectId,
        delivery.files.map((file) => ({ path: file.path })),
      );
      if (result.errors.length > 0 && result.restored.length === 0 && result.removed.length === 0) {
        setError(result.errors.join("\n"));
        return;
      }
      if (result.errors.length > 0) {
        setError(result.errors.join("\n"));
      }
      onUndone?.();
      onRestored?.();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div
        className={`event-card event-card--turn_delivery${open ? " is-open" : ""}${undone ? " is-undone" : ""}`}
      >
        <div className="turn-delivery__bar">
          <button
            type="button"
            className="turn-delivery__toggle"
            title={title}
            aria-expanded={open}
            onClick={() => setOpen((value) => !value)}
          >
            <span className={`turn-delivery__fold${open ? " is-open" : ""}`} aria-hidden>
              ▸
            </span>
            <span className="turn-delivery__headline">
              {headline}
              {undone ? " · 已撤销" : ""}
            </span>
            {delivery.totalDiff && (delivery.totalDiff.add > 0 || delivery.totalDiff.del > 0) && (
              <span className="turn-delivery__totals">
                {delivery.totalDiff.add > 0 && (
                  <span className="turn-delivery__stat turn-delivery__stat--add">
                    +{delivery.totalDiff.add}
                  </span>
                )}
                {delivery.totalDiff.del > 0 && (
                  <span className="turn-delivery__stat turn-delivery__stat--del">
                    −{delivery.totalDiff.del}
                  </span>
                )}
              </span>
            )}
          </button>
          {canUndo && (
            <button
              type="button"
              className="turn-delivery__undo"
              title="撤销本轮对工作区的改动"
              disabled={busy}
              onClick={() => setConfirming(true)}
            >
              <Undo2 size={12} strokeWidth={2.25} aria-hidden />
              撤销
            </button>
          )}
        </div>
        {error && <div className="turn-delivery__error">{error}</div>}
        {open && (
          <div className="turn-delivery">
            {fileCount > 0 ? (
              <div className="turn-delivery__files">
                {delivery.files.map((file) => (
                  <DeliveryFileRow key={file.path} file={file} projectId={projectId} />
                ))}
              </div>
            ) : null}
            {delivery.tally && <div className="turn-delivery__tally">{delivery.tally}</div>}
          </div>
        )}
      </div>
      {confirming && (
        <ConfirmDialog
          title="撤销本轮改动"
          itemName={headline}
          description="这些文件会还原到 git HEAD。本轮之后、还没提交的修改也会一起丢掉。"
          confirmLabel={busy ? "撤销中…" : "撤销"}
          cancelLabel="取消"
          danger
          onCancel={() => {
            if (!busy) setConfirming(false);
          }}
          onConfirm={() => {
            setConfirming(false);
            void runUndo();
          }}
        />
      )}
    </>
  );
}
