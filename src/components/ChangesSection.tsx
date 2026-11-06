import { ExternalLink, FolderOpen, GitCommitHorizontal, RefreshCw } from "lucide-react";
import { memo, useState } from "react";
import { openExternal, revealInFileManager, resolveLinkTarget } from "../lib/api";
import { useT } from "../lib/i18n";
import { isDocPath, openInDocTabIfEligible } from "../lib/openFile";
import { ConfirmDialog } from "./ConfirmDialog";
import type { ChangedFile } from "../lib/types";

function changeLetter(changeType: ChangedFile["changeType"]): string {
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

function parentRelativePath(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const slash = normalized.lastIndexOf("/");
  if (slash <= 0) return ".";
  return normalized.slice(0, slash);
}

function Delta({ added, deleted }: { added?: number | null; deleted?: number | null }) {
  if (added == null && deleted == null) return null;
  return (
    <>
      {added != null && added > 0 && <span className="rail-plus">+{added}</span>}
      {added != null && added > 0 && deleted != null && deleted > 0 && " "}
      {deleted != null && deleted > 0 && <span className="rail-minus">−{deleted}</span>}
    </>
  );
}

function ChangedFileRow({
  file,
  cwd,
  onOpenDiff,
  onRisky,
  onNote,
}: {
  file: ChangedFile;
  cwd: string | null;
  onOpenDiff?: (path: string) => void;
  onRisky: (path: string) => void;
  onNote: (note: string | null) => void;
}) {
  const t = useT();
  const deleted = file.changeType === "deleted";
  const canAct = Boolean(cwd);

  const openFile = async () => {
    if (!cwd || deleted) return;
    onNote(null);
    // Markdown/text renders in the in-app document tab; other files keep the
    // OS-handler route (risky ones still take the confirm path below).
    if (isDocPath(file.path)) {
      const resolution = await resolveLinkTarget(file.path, cwd).catch(() => null);
      if (resolution?.kind === "file" && !resolution.risky && openInDocTabIfEligible(resolution.target)) {
        return;
      }
    }
    try {
      const result = await openExternal(file.path, cwd);
      if (result.reason === "risky") {
        onRisky(file.path);
        return;
      }
      if (!result.opened) onNote(result.message ?? t("context.cannotOpenFile"));
    } catch (error) {
      onNote(error instanceof Error ? error.message : String(error));
    }
  };

  const reveal = async () => {
    if (!cwd) return;
    onNote(null);
    const target = deleted ? parentRelativePath(file.path) : file.path;
    try {
      await revealInFileManager(target, cwd);
    } catch (error) {
      onNote(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <li className="rail-ln rail-file">
      <button
        type="button"
        className="rail-file__main"
        title={t("context.viewDiff", { path: file.path })}
        onClick={() => onOpenDiff?.(file.path)}
      >
        <span className={`rail-file__badge is-${file.changeType}`}>{changeLetter(file.changeType)}</span>
        <span className="rail-file__path">{file.path}</span>
        <span className="rail-file__delta">
          <Delta added={file.additions} deleted={file.deletions} />
        </span>
      </button>
      <span className="rail-file__actions">
        <button
          type="button"
          className="rail-ib rail-ib--sm"
          title={deleted ? t("context.fileDeleted") : t("context.openDefault")}
          aria-label={deleted ? t("context.fileDeleted") : t("context.openDefault")}
          disabled={!canAct || deleted}
          onClick={() => void openFile()}
        >
          <ExternalLink size={11} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="rail-ib rail-ib--sm"
          title={deleted ? t("context.openParentFolder") : t("context.revealInExplorer")}
          aria-label={deleted ? t("context.openParentFolder") : t("context.revealInExplorer")}
          disabled={!canAct}
          onClick={() => void reveal()}
        >
          <FolderOpen size={11} strokeWidth={1.5} />
        </button>
      </span>
    </li>
  );
}

type ChangesSectionProps = {
  changedFiles?: ChangedFile[];
  changedFilesNote?: string | null;
  onRefresh?: () => void;
  gitBranch?: string | null;
  onCommitAndPush?: () => void;
  onOpenDiff?: (path: string) => void;
  projectRoot?: string | null;
};

const NO_FILES: ChangedFile[] = [];

export const ChangesSection = memo(function ChangesSection({
  changedFiles = NO_FILES,
  changedFilesNote = null,
  onRefresh,
  gitBranch = null,
  onCommitAndPush,
  onOpenDiff,
  projectRoot = null,
}: ChangesSectionProps) {
  const t = useT();
  const [riskyOpenPath, setRiskyOpenPath] = useState<string | null>(null);
  const [fileActionNote, setFileActionNote] = useState<string | null>(null);

  const counted = changedFiles.some((f) => f.additions != null || f.deletions != null);
  const added = changedFiles.reduce((sum, f) => sum + (f.additions ?? 0), 0);
  const deleted = changedFiles.reduce((sum, f) => sum + (f.deletions ?? 0), 0);

  return (
    <section className="rail-sec" id="context-changes" aria-label={t("rail.changes")}>
      <div className="rail-sh">
        <span>{t("rail.changes")}</span>
        {changedFiles.length > 0 && (
          <span className="rail-sh__meta">
            {counted ? <Delta added={added} deleted={deleted} /> : t("rail.changesCount", { count: changedFiles.length })}
          </span>
        )}
        <span className="rail-act">
          {onCommitAndPush && changedFiles.length > 0 && (
            <button
              type="button"
              className="rail-ib"
              title={t("rail.commitPush")}
              aria-label={t("rail.commitPush")}
              onClick={onCommitAndPush}
            >
              <GitCommitHorizontal size={13} strokeWidth={1.5} />
            </button>
          )}
          {onRefresh && (
            <button
              type="button"
              className="rail-ib"
              title={t("rail.refreshChanges")}
              aria-label={t("rail.refreshChanges")}
              onClick={onRefresh}
            >
              <RefreshCw size={12} strokeWidth={1.5} />
            </button>
          )}
        </span>
      </div>
      {gitBranch ? (
        <div className="rail-ph rail-branch" title={gitBranch}>
          {t("rail.branch", { name: gitBranch })}
        </div>
      ) : null}
      {changedFiles.length === 0 ? (
        <p className="rail-empty">{changedFilesNote ?? t("rail.noChanges")}</p>
      ) : (
        <ul className="rail-list">
          {changedFiles.map((file) => (
            <ChangedFileRow
              key={`${file.changeType}:${file.path}`}
              file={file}
              cwd={projectRoot}
              onOpenDiff={onOpenDiff}
              onRisky={setRiskyOpenPath}
              onNote={setFileActionNote}
            />
          ))}
        </ul>
      )}
      {fileActionNote ? <p className="rail-empty">{fileActionNote}</p> : null}

      {riskyOpenPath && (
        <ConfirmDialog
          title={t("context.openExecutableTitle")}
          itemName={riskyOpenPath}
          description={t("context.openExecutableDesc")}
          confirmLabel={t("common.open")}
          cancelLabel={t("common.cancel")}
          onCancel={() => setRiskyOpenPath(null)}
          onConfirm={() => {
            const path = riskyOpenPath;
            setRiskyOpenPath(null);
            if (!path || !projectRoot) return;
            void openExternal(path, projectRoot, true).catch((error) => {
              setFileActionNote(error instanceof Error ? error.message : String(error));
            });
          }}
        />
      )}
    </section>
  );
});
