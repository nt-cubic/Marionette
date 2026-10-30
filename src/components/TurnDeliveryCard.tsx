import { useEffect, useState } from "react";
import { ChevronDown, ExternalLink, FolderOpen, Undo2 } from "lucide-react";
import {
  CHAT_PROJECT_ID,
  getFileDiff,
  openExternal,
  restoreTurnFiles,
  revealInFileManager,
} from "../lib/api";
import type { TurnDelivery, TurnDeliveryChangeType, TurnDeliveryFile } from "../lib/turnDelivery";
import { formatDeliveryHeadline } from "../lib/turnDelivery";
import { showToast } from "../lib/toast";
import { useT } from "../lib/i18n";
import { ConfirmDialog } from "./ConfirmDialog";
import { UnifiedDiffView } from "./UnifiedDiffView";

/** A long turn lists this many files, then "其余 N 个" — the transcript stays readable. */
const FILES_SHOWN = 8;

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

/** `src/lib/api.ts` → dir `src/lib/`, name `api.ts`: the name is what you scan for. */
function splitPath(relPath: string): { dir: string; name: string } {
  const norm = relPath.replace(/\\/g, "/");
  const cut = norm.lastIndexOf("/");
  return cut < 0 ? { dir: "", name: norm } : { dir: norm.slice(0, cut + 1), name: norm.slice(cut + 1) };
}

/** Folder of a workspace path — the reveal target for a deleted file. */
function parentDir(path: string): string {
  const norm = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const cut = norm.lastIndexOf("/");
  return cut <= 0 ? norm : norm.slice(0, cut);
}

/** Add/del split of changed lines as one thin two-tone bar (green, then red). */
function DiffRatio({ add, del }: { add: number; del: number }) {
  const total = add + del;
  if (total === 0) return null;
  return (
    <span className="turn-delivery__ratio" aria-hidden>
      <i className="is-add" style={{ width: `${(add / total) * 100}%` }} />
      <i className="is-del" style={{ width: `${(del / total) * 100}%` }} />
    </span>
  );
}

function DeliveryFileRow({
  file,
  projectId,
  cwd,
}: {
  file: TurnDeliveryFile;
  projectId?: string;
  cwd?: string | null;
}) {
  const [expanded, setExpanded] = useState(false);
  const [diff, setDiff] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [riskyConfirm, setRiskyConfirm] = useState(false);
  const t = useT();

  const canDiff = Boolean(projectId) && projectId !== CHAT_PROJECT_ID && Boolean(file.path);
  const deleted = file.changeType === "deleted";

  const openFile = async (force = false) => {
    if (deleted) return;
    try {
      const result = await openExternal(file.path, cwd, force);
      if (result.reason === "risky") {
        setRiskyConfirm(true);
        return;
      }
      if (!result.opened) {
        showToast({ text: result.message ?? t("turn.openFileFailed"), tone: "error" });
      }
    } catch (err) {
      showToast({ text: err instanceof Error ? err.message : String(err), tone: "error" });
    }
  };

  const reveal = async () => {
    try {
      await revealInFileManager(deleted ? parentDir(file.path) : file.path, cwd);
    } catch (err) {
      showToast({ text: err instanceof Error ? err.message : String(err), tone: "error" });
    }
  };

  useEffect(() => {
    if (!expanded) return;
    if (!projectId || projectId === CHAT_PROJECT_ID || !file.path) {
      setError(t("turn.noProjectContext"));
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
      timeoutId = window.setTimeout(() => reject(new Error(t("turn.diffTimeout"))), 8000);
    });
    void Promise.race([getFileDiff(projectId, file.path), timeout])
      .then((text) => {
        if (!cancelled) setDiff(text.trim() ? text : t("turn.noChanges"));
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

  const path = splitPath(file.relPath);
  const title =
    file.touched > 1 ? t("turn.editedTimes", { path: file.path, n: file.touched }) : file.path;

  return (
    <>
      <div className={`turn-delivery__row${canDiff ? " is-expandable" : ""}`}>
        {file.changeType ? (
          <span className={`change-badge change-${file.changeType}`}>
            {badgeLetter(file.changeType)}
          </span>
        ) : (
          <span className="turn-delivery__dot" aria-hidden />
        )}
        <button
          type="button"
          className="turn-delivery__main"
          title={canDiff ? `${title}\n${t("turn.clickToViewDiff")}` : title}
          onClick={canDiff ? () => setExpanded((value) => !value) : () => void openFile()}
          aria-expanded={canDiff ? expanded : undefined}
        >
          <span className="turn-delivery__path">
            {path.dir && <span className="turn-delivery__dir">{path.dir}</span>}
            {path.name}
          </span>
          <span className="turn-delivery__stats">
            {file.add != null && file.add > 0 && (
              <span className="turn-delivery__stat turn-delivery__stat--add">+{file.add}</span>
            )}
            {file.del != null && file.del > 0 && (
              <span className="turn-delivery__stat turn-delivery__stat--del">−{file.del}</span>
            )}
            <DiffRatio add={file.add ?? 0} del={file.del ?? 0} />
          </span>
          {canDiff && (
            <ChevronDown
              size={12}
              className={`turn-delivery__chevron${expanded ? " is-open" : ""}`}
              aria-hidden
            />
          )}
        </button>
        <div className="turn-delivery__actions">
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm turn-delivery__action"
            title={deleted ? t("turn.fileDeleted") : t("turn.openWithDefault")}
            aria-label={deleted ? t("turn.fileDeleted") : t("turn.openWithDefault")}
            disabled={deleted}
            onClick={() => void openFile()}
          >
            <ExternalLink size={11} />
          </button>
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm turn-delivery__action"
            title={deleted ? t("turn.openParentFolder") : t("turn.revealInExplorer")}
            aria-label={deleted ? t("turn.openParentFolder") : t("turn.revealInExplorer")}
            onClick={() => void reveal()}
          >
            <FolderOpen size={11} />
          </button>
        </div>
      </div>
      {riskyConfirm && (
        <ConfirmDialog
          title={t("turn.openExecutableTitle")}
          itemName={file.path}
          description={t("turn.openExecutableDesc")}
          confirmLabel={t("common.open")}
          cancelLabel={t("common.cancel")}
          onCancel={() => setRiskyConfirm(false)}
          onConfirm={() => {
            setRiskyConfirm(false);
            void openFile(true);
          }}
        />
      )}
      {expanded && (
        <div className="turn-delivery__diff">
          {loading && diff === null && (
            <div className="turn-delivery__diff-note">{t("turn.loadingDiff")}</div>
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
  cwd,
  undone = false,
  onUndone,
  onRestored,
}: {
  delivery: TurnDelivery;
  projectId?: string;
  /** Session working dir (project root) — resolves file paths for 打开/显示. */
  cwd?: string | null;
  undone?: boolean;
  onUndone?: () => void;
  onRestored?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const t = useT();

  const headline = formatDeliveryHeadline(delivery);
  const fileCount = delivery.files.length;
  const canUndo =
    !undone &&
    !busy &&
    fileCount > 0 &&
    Boolean(projectId) &&
    projectId !== CHAT_PROJECT_ID;
  const title = delivery.files.map((f) => f.relPath).join("\n");
  // Closed card still says *which* files: the first two names, then a count.
  const peekNames = delivery.files.slice(0, 2).map((f) => splitPath(f.relPath).name);
  const peek =
    peekNames.length === 0
      ? ""
      : `${peekNames.join(t("turn.nameSeparator"))}${fileCount > 2 ? t("turn.moreCount", { count: fileCount }) : ""}`;

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
      const reverted = result.restored.length + result.removed.length;
      showToast({
        text: t("turn.undoneToast", { count: reverted }),
        detail: result.errors.length > 0 ? t("turn.undonePartial", { count: result.errors.length }) : undefined,
        tone: result.errors.length > 0 ? "error" : "ok",
      });
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
              {undone ? ` · ${t("turn.undone")}` : ""}
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
                <DiffRatio add={delivery.totalDiff.add} del={delivery.totalDiff.del} />
              </span>
            )}
            {!open && peek && <span className="turn-delivery__peek">{peek}</span>}
          </button>
          {canUndo && (
            <button
              type="button"
              className="turn-delivery__undo"
              title={t("turn.undoTitle")}
              disabled={busy}
              onClick={() => setConfirming(true)}
            >
              <Undo2 size={12} strokeWidth={2.25} aria-hidden />
              {t("turn.undo")}
            </button>
          )}
        </div>
        {error && <div className="turn-delivery__error">{error}</div>}
        {open && (
          <div className="turn-delivery">
            {fileCount > 0 ? (
              <div className="turn-delivery__files">
                {(showAll ? delivery.files : delivery.files.slice(0, FILES_SHOWN)).map((file) => (
                  <DeliveryFileRow key={file.path} file={file} projectId={projectId} cwd={cwd} />
                ))}
                {!showAll && fileCount > FILES_SHOWN && (
                  <button type="button" className="turn-delivery__more" onClick={() => setShowAll(true)}>
                    {t("turn.remainingFiles", { count: fileCount - FILES_SHOWN })}
                  </button>
                )}
              </div>
            ) : null}
            {delivery.tally && <div className="turn-delivery__tally">{delivery.tally}</div>}
          </div>
        )}
      </div>
      {confirming && (
        <ConfirmDialog
          title={t("turn.undoTurnTitle")}
          itemName={headline}
          description={t("turn.undoTurnDesc")}
          confirmLabel={busy ? t("turn.undoing") : t("turn.undo")}
          cancelLabel={t("common.cancel")}
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
