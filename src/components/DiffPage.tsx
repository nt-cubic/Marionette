import { FileDiff, FolderOpen, GitCommitHorizontal, RefreshCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import { getFileDiff, revealInFileManager } from "../lib/api";
import { useT } from "../lib/i18n";
import type { ChangedFile } from "../lib/types";
import { UnifiedDiffView } from "./UnifiedDiffView";

type DiffPageProps = {
  projectId: string | null;
  projectRoot: string | null;
  files: ChangedFile[];
  branch: string | null;
  /** File shown on the right; null picks the first changed file. */
  selectedPath: string | null;
  onSelect: (path: string) => void;
  onClose: () => void;
  onRefresh?: () => void;
  onCommitAndPush?: () => void;
};

const MARK: Record<ChangedFile["changeType"], string> = {
  modified: "M",
  added: "A",
  untracked: "A",
  deleted: "D",
};

function joinPath(root: string | null, rel: string): string {
  if (!root) return rel;
  return `${root.replace(/[\\/]+$/, "")}/${rel}`;
}

/**
 * 「改动」伪标签页：左列改动文件、右侧该文件相对 HEAD 的 diff。
 * Replaces the diff dialog in the main window; the dock strip and detached
 * windows keep the dialog (too narrow for two columns).
 */
export function DiffPage({
  projectId,
  projectRoot,
  files,
  branch,
  selectedPath,
  onSelect,
  onClose,
  onRefresh,
  onCommitAndPush,
}: DiffPageProps) {
  const t = useT();
  const current =
    (selectedPath && files.find((file) => file.path === selectedPath)) || files[0] || null;
  const currentPath = current?.path ?? selectedPath ?? null;
  const [diff, setDiff] = useState<{ path: string; text: string } | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!projectId || !currentPath) {
      setDiff(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    void getFileDiff(projectId, currentPath)
      .then((text) => {
        if (!cancelled) setDiff({ path: currentPath, text });
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, currentPath, files]);

  const totals = files.reduce(
    (acc, file) => ({
      add: acc.add + (file.additions ?? 0),
      del: acc.del + (file.deletions ?? 0),
      known: acc.known || file.additions != null || file.deletions != null,
    }),
    { add: 0, del: 0, known: false },
  );
  const slash = currentPath ? Math.max(currentPath.lastIndexOf("/"), currentPath.lastIndexOf("\\")) : -1;
  const fileName = currentPath ? currentPath.slice(slash + 1) : "";
  const fileDir = currentPath && slash > 0 ? currentPath.slice(0, slash + 1) : "";

  return (
    <div className="diff-page">
      <div className="diff-page__bar">
        <span className="diff-page__scope">{t("v2pages.diffScope")}</span>
        <span className="diff-page__summary">
          {t("v2pages.diffFiles", { count: files.length })}
          {totals.known ? (
            <>
              {" "}
              <span className="diff-page__add">+{totals.add}</span>{" "}
              <span className="diff-page__del">−{totals.del}</span>
            </>
          ) : null}
          {branch ? <span className="diff-page__branch">{t("v2pages.diffBranch", { branch })}</span> : null}
        </span>
        <div className="diff-page__actions">
          {onRefresh ? (
            <button type="button" className="diff-page__icon" title={t("v2pages.diffRefresh")} aria-label={t("v2pages.diffRefresh")} onClick={onRefresh}>
              <RefreshCw size={13} />
            </button>
          ) : null}
          {onCommitAndPush ? (
            <button type="button" className="diff-page__text-btn" title={t("v2pages.diffCommit")} onClick={onCommitAndPush}>
              <GitCommitHorizontal size={13} aria-hidden />
              <span className="diff-page__btn-label">{t("v2pages.diffCommit")}</span>
            </button>
          ) : null}
          <button type="button" className="diff-page__icon" title={t("common.close")} aria-label={t("common.close")} onClick={onClose}>
            <X size={13} />
          </button>
        </div>
      </div>

      <div className="diff-page__body">
        <ul className="diff-page__files" aria-label={t("v2pages.diffFileList")}>
          {files.length === 0 ? <li className="diff-page__empty">{t("v2pages.diffNoFiles")}</li> : null}
          {files.map((file) => (
            <li key={file.path}>
              <button
                type="button"
                className={`diff-page__file${file.path === currentPath ? " is-active" : ""}`}
                title={file.path}
                onClick={() => onSelect(file.path)}
              >
                <span className={`diff-page__mark is-${file.changeType}`}>{MARK[file.changeType]}</span>
                <span className="diff-page__path">{file.path}</span>
                {file.additions != null || file.deletions != null ? (
                  <span className="diff-page__delta">
                    {file.additions ? <span className="diff-page__add">+{file.additions}</span> : null}
                    {file.deletions ? <span className="diff-page__del"> −{file.deletions}</span> : null}
                  </span>
                ) : null}
              </button>
            </li>
          ))}
        </ul>

        <section className="diff-page__view" aria-live="polite">
          {currentPath ? (
            <>
              <div className="diff-page__file-head">
                <FileDiff size={13} aria-hidden className="diff-page__file-icon" />
                <span className="diff-page__file-name">{fileName}</span>
                <span className="diff-page__file-dir">{fileDir}</span>
                {projectRoot ? (
                  <button
                    type="button"
                    className="diff-page__text-btn"
                    title={t("v2pages.diffReveal")}
                    onClick={() => void revealInFileManager(joinPath(projectRoot, currentPath))}
                  >
                    <FolderOpen size={13} aria-hidden />
                    <span className="diff-page__btn-label">{t("v2pages.diffReveal")}</span>
                  </button>
                ) : null}
              </div>
              {loading && diff?.path !== currentPath ? (
                <p className="diff-page__empty">{t("v2pages.diffLoading")}</p>
              ) : diff && diff.text.trim() ? (
                <UnifiedDiffView className="diff-page__diff" text={diff.text} elideUnchanged />
              ) : (
                <p className="diff-page__empty">{t("v2pages.diffEmpty")}</p>
              )}
            </>
          ) : (
            <p className="diff-page__empty">{t("v2pages.diffNoFiles")}</p>
          )}
        </section>
      </div>
    </div>
  );
}
