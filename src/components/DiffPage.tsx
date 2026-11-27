import {
  ChevronDown,
  ChevronUp,
  FileDiff,
  FolderOpen,
  GitCommitHorizontal,
  Palette,
  Pilcrow,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getFileDiff, revealInFileManager } from "../lib/api";
import {
  buildDiffDisplay,
  findDiffMatches,
  type DiffMatch,
} from "../lib/diffPresentation";
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

const IGNORE_WS_KEY = "marionette-diff-ignore-whitespace";
const COLORBLIND_KEY = "marionette-diff-colorblind";

function readBoolPref(key: string): boolean {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

function writeBoolPref(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, value ? "1" : "0");
  } catch {
    // Private mode / storage disabled: the toggle still works this session.
  }
}

function joinPath(root: string | null, rel: string): string {
  if (!root) return rel;
  return `${root.replace(/[\\/]+$/, "")}/${rel}`;
}

/**
 * The "changes" pseudo-tab: changed files on the left, the selected file's
 * diff against HEAD on the right.
 *
 * Find, ignore-whitespace and the colourblind palette are presentation only:
 * the fetched diff text, review comments and commit data are never rewritten.
 * Find searches every diff this page has loaded (and switches files when a
 * match lives beyond the current one).
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

  // Loaded diff texts by path. Loading is lazy (only files the user opened),
  // which is exactly the set find is allowed to search.
  const [diffs, setDiffs] = useState<Record<string, string>>({});
  const [loadingPath, setLoadingPath] = useState<string | null>(null);
  const filesRef = useRef(files);
  const projectRef = useRef(projectId);
  const pendingRef = useRef<Set<string>>(new Set());
  const cacheEpochRef = useRef(0);
  const [ignoreWhitespace, setIgnoreWhitespace] = useState(() =>
    readBoolPref(IGNORE_WS_KEY),
  );
  const [colorblind, setColorblind] = useState(() => readBoolPref(COLORBLIND_KEY));

  // A refreshed file list or another project may describe different content;
  // drop the cache. Find may only ever search what is really loaded.
  useEffect(() => {
    if (filesRef.current === files && projectRef.current === projectId) return;
    filesRef.current = files;
    projectRef.current = projectId;
    cacheEpochRef.current += 1;
    pendingRef.current.clear();
    setDiffs({});
  }, [files, projectId]);

  useEffect(() => {
    if (!projectId || !currentPath) return;
    if (diffs[currentPath] !== undefined || pendingRef.current.has(currentPath)) return;
    const epoch = cacheEpochRef.current;
    pendingRef.current.add(currentPath);
    setLoadingPath(currentPath);
    void getFileDiff(projectId, currentPath)
      .then((text) => {
        if (cacheEpochRef.current === epoch) {
          setDiffs((previous) => ({ ...previous, [currentPath]: text }));
        }
      })
      .finally(() => {
        pendingRef.current.delete(currentPath);
        if (cacheEpochRef.current === epoch) {
          setLoadingPath((path) => (path === currentPath ? null : path));
        }
      });
  }, [currentPath, diffs, projectId]);

  useEffect(() => {
    writeBoolPref(IGNORE_WS_KEY, ignoreWhitespace);
  }, [ignoreWhitespace]);
  useEffect(() => {
    writeBoolPref(COLORBLIND_KEY, colorblind);
  }, [colorblind]);

  // -- Find over the loaded files ----------------------------------------
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [findHit, setFindHit] = useState(0);
  const findInputRef = useRef<HTMLInputElement>(null);

  const groups = useMemo(() => {
    if (!findQuery.trim()) return [] as { path: string; matches: DiffMatch[] }[];
    return files
      .map((file) => ({ path: file.path, text: diffs[file.path] }))
      .filter(
        (entry): entry is { path: string; text: string } => entry.text !== undefined,
      )
      .map(({ path, text }) => {
        const display = buildDiffDisplay(text, {
          ignoreWhitespace,
          elideUnchanged: true,
        });
        return { path, matches: findDiffMatches(display.lines, findQuery) };
      })
      .filter((group) => group.matches.length > 0);
  }, [diffs, files, findQuery, ignoreWhitespace]);

  const flatMatches = useMemo(
    () =>
      groups.flatMap((group) =>
        group.matches.map((match) => ({ path: group.path, match })),
      ),
    [groups],
  );
  const totalMatches = flatMatches.length;
  const activeIndex = totalMatches
    ? ((findHit % totalMatches) + totalMatches) % totalMatches
    : 0;
  const activeFlat = totalMatches ? flatMatches[activeIndex] : null;

  const closeFind = useCallback(() => {
    setFindOpen(false);
    setFindQuery("");
    setFindHit(0);
  }, []);

  const stepFind = useCallback(
    (delta: number) => {
      if (!totalMatches) return;
      setFindHit((currentHit) => {
        const next = currentHit + delta;
        return ((next % totalMatches) + totalMatches) % totalMatches;
      });
    },
    [totalMatches],
  );

  // A match in another loaded file: switch to it so the highlight is visible.
  // The row list for that file is already in memory, so this is instant.
  useEffect(() => {
    if (!findOpen || !activeFlat) return;
    if (activeFlat.path !== currentPath) onSelect(activeFlat.path);
  }, [activeFlat?.path, currentPath, findOpen, onSelect]);

  useEffect(() => {
    setFindHit(0);
  }, [findQuery]);

  useEffect(() => {
    if (!findOpen) return;
    const input = findInputRef.current;
    if (input) {
      input.focus();
      input.select();
    }
  }, [findOpen]);

  // Ctrl F belongs to this page while it is mounted (it replaces the center).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && findOpen) {
        event.preventDefault();
        event.stopImmediatePropagation();
        closeFind();
        return;
      }
      if (!(event.ctrlKey || event.metaKey) || event.shiftKey || event.altKey) return;
      if (event.key !== "f" && event.key !== "F") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      setFindOpen(true);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [closeFind, findOpen]);

  const activeMatch =
    activeFlat && activeFlat.path === currentPath ? activeFlat.match : null;

  const totals = files.reduce(
    (acc, file) => ({
      add: acc.add + (file.additions ?? 0),
      del: acc.del + (file.deletions ?? 0),
      known: acc.known || file.additions != null || file.deletions != null,
    }),
    { add: 0, del: 0, known: false },
  );
  const slash = currentPath
    ? Math.max(currentPath.lastIndexOf("/"), currentPath.lastIndexOf("\\"))
    : -1;
  const fileName = currentPath ? currentPath.slice(slash + 1) : "";
  const fileDir = currentPath && slash > 0 ? currentPath.slice(0, slash + 1) : "";
  const currentLoaded = currentPath ? diffs[currentPath] : undefined;

  return (
    <div className={`diff-page${colorblind ? " is-colorblind" : ""}`}>
      <div className="diff-page__bar">
        <span className="diff-page__scope">{t("v2pages.diffScope")}</span>
        <span className="diff-page__summary">
          {t("v2pages.diffFiles", { count: files.length })}
          {totals.known ? (
            <>
              {" "}
              <span className="diff-page__add">+{totals.add}</span>{" "}
              <span className="diff-page__del">
                {"\u2212"}
                {totals.del}
              </span>
            </>
          ) : null}
          {branch ? (
            <span className="diff-page__branch">
              {t("v2pages.diffBranch", { branch })}
            </span>
          ) : null}
        </span>
        <div className="diff-page__actions">
          <button
            type="button"
            className={`diff-page__icon${findOpen ? " is-active" : ""}`}
            title={t("v2pages.diffFindOpen")}
            aria-label={t("v2pages.diffFindOpen")}
            aria-pressed={findOpen}
            onClick={() => (findOpen ? closeFind() : setFindOpen(true))}
          >
            <Search size={13} />
          </button>
          <button
            type="button"
            className={`diff-page__icon${ignoreWhitespace ? " is-active" : ""}`}
            title={t("v2pages.diffIgnoreWhitespace")}
            aria-label={t("v2pages.diffIgnoreWhitespace")}
            aria-pressed={ignoreWhitespace}
            onClick={() => setIgnoreWhitespace((value) => !value)}
          >
            <Pilcrow size={13} />
          </button>
          <button
            type="button"
            className={`diff-page__icon${colorblind ? " is-active" : ""}`}
            title={t("v2pages.diffColorblind")}
            aria-label={t("v2pages.diffColorblind")}
            aria-pressed={colorblind}
            onClick={() => setColorblind((value) => !value)}
          >
            <Palette size={13} />
          </button>
          {onRefresh ? (
            <button
              type="button"
              className="diff-page__icon"
              title={t("v2pages.diffRefresh")}
              aria-label={t("v2pages.diffRefresh")}
              onClick={() => {
                cacheEpochRef.current += 1;
                pendingRef.current.clear();
                setDiffs({});
                onRefresh();
              }}
            >
              <RefreshCw size={13} />
            </button>
          ) : null}
          {onCommitAndPush ? (
            <button
              type="button"
              className="diff-page__text-btn"
              title={t("v2pages.diffCommit")}
              onClick={onCommitAndPush}
            >
              <GitCommitHorizontal size={13} aria-hidden />
              <span className="diff-page__btn-label">{t("v2pages.diffCommit")}</span>
            </button>
          ) : null}
          <button
            type="button"
            className="diff-page__icon"
            title={t("common.close")}
            aria-label={t("common.close")}
            onClick={onClose}
          >
            <X size={13} />
          </button>
        </div>
      </div>

      {findOpen && (
        <div className="find-bar find-bar--inline diff-page__find" role="search">
          <Search size={12} aria-hidden />
          <input
            ref={findInputRef}
            className="find-bar__input"
            type="text"
            value={findQuery}
            placeholder={t("v2pages.diffFindPlaceholder")}
            aria-label={t("v2pages.diffFindPlaceholder")}
            spellCheck={false}
            onChange={(event) => setFindQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                closeFind();
              } else if (event.key === "Enter") {
                event.preventDefault();
                event.stopPropagation();
                stepFind(event.shiftKey ? -1 : 1);
              }
            }}
          />
          <span
            className={`find-bar__count${
              findQuery.trim() && totalMatches === 0 ? " is-empty" : ""
            }`}
          >
            {findQuery.trim()
              ? totalMatches > 0
                ? t("v2pages.diffFindCounter", {
                    i: activeIndex + 1,
                    n: totalMatches,
                  })
                : t("v2pages.diffFindNoMatch")
              : ""}
          </span>
          <button
            type="button"
            className="find-bar__btn"
            title={t("v2pages.diffFindPrev")}
            aria-label={t("v2pages.diffFindPrev")}
            disabled={totalMatches === 0}
            onClick={() => stepFind(-1)}
          >
            <ChevronUp size={12} aria-hidden />
          </button>
          <button
            type="button"
            className="find-bar__btn"
            title={t("v2pages.diffFindNext")}
            aria-label={t("v2pages.diffFindNext")}
            disabled={totalMatches === 0}
            onClick={() => stepFind(1)}
          >
            <ChevronDown size={12} aria-hidden />
          </button>
          <button
            type="button"
            className="find-bar__btn"
            title={t("v2pages.diffFindClose")}
            aria-label={t("v2pages.diffFindClose")}
            onClick={closeFind}
          >
            <X size={12} aria-hidden />
          </button>
        </div>
      )}

      <div className="diff-page__body">
        <ul className="diff-page__files" aria-label={t("v2pages.diffFileList")}>
          {files.length === 0 ? (
            <li className="diff-page__empty">{t("v2pages.diffNoFiles")}</li>
          ) : null}
          {files.map((file) => (
            <li key={file.path}>
              <button
                type="button"
                className={`diff-page__file${file.path === currentPath ? " is-active" : ""}`}
                title={file.path}
                onClick={() => onSelect(file.path)}
              >
                <span className={`diff-page__mark is-${file.changeType}`}>
                  {MARK[file.changeType]}
                </span>
                <span className="diff-page__path">{file.path}</span>
                {file.additions != null || file.deletions != null ? (
                  <span className="diff-page__delta">
                    {file.additions ? (
                      <span className="diff-page__add">+{file.additions}</span>
                    ) : null}
                    {file.deletions ? (
                      <span className="diff-page__del">
                        {" \u2212"}
                        {file.deletions}
                      </span>
                    ) : null}
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
                    onClick={() =>
                      void revealInFileManager(joinPath(projectRoot, currentPath))
                    }
                  >
                    <FolderOpen size={13} aria-hidden />
                    <span className="diff-page__btn-label">{t("v2pages.diffReveal")}</span>
                  </button>
                ) : null}
              </div>
              {loadingPath === currentPath && currentLoaded === undefined ? (
                <p className="diff-page__empty">{t("v2pages.diffLoading")}</p>
              ) : currentLoaded !== undefined && currentLoaded.trim() ? (
                <UnifiedDiffView
                  className="diff-page__diff"
                  text={currentLoaded}
                  elideUnchanged
                  ignoreWhitespace={ignoreWhitespace}
                  findQuery={findQuery}
                  activeMatch={activeMatch}
                />
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
