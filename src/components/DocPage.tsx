import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  Columns2,
  ExternalLink,
  Eye,
  FileText,
  MessageSquarePlus,
  PencilLine,
  Save,
  X,
} from "lucide-react";
import { openExternal, readDocFile, saveDocFile } from "../lib/api";
import { showToast } from "../lib/toast";
import { useT } from "../lib/i18n";
import { ConfirmDialog } from "./ConfirmDialog";
import { LinkCwdContext } from "./LinkedText";
import { DocNavContext, MarkdownBody, MarkdownImageCwdContext } from "./MarkdownBody";

/**
 * In-app document tab: markdown/text rendered (and editable) inside Marionette
 * instead of being handed to whatever the OS registered for `.md`.
 *
 * Two modes around one draft: preview reuses the transcript markdown pipeline
 * (math/mermaid/shiki/spec cards all included, images resolved against the
 * document's own directory), edit is a monospace source editor. Saving carries
 * an mtime optimistic lock — an external modification is a confirm, never a
 * silent overwrite. "Quote to agent" turns a selection into a cited block in
 * the composer; that is the feedback loop (anchored comments are deliberately
 * not built).
 */

type DocPageProps = {
  /** Absolute path (the opener resolved it; readDocFile near-miss may adjust). */
  path: string;
  onClose: () => void;
  /** Prefill the composer with a cited quote block. */
  onQuote: (block: string) => void;
  /** Open another document tab (relative `.md` links inside this document). */
  onNavigate: (absolutePath: string) => void;
  /** Compare-with-chat toggle only exists in the main window's page mount. */
  compareAvailable?: boolean;
  compareActive?: boolean;
  onToggleCompare?: () => void;
  /** Esc closes the page — false in the dock full-page mount (chrome owns Esc). */
  escToClose?: boolean;
};

function fileNameOf(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

function parentDirOf(path: string): string {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut > 0 ? path.slice(0, cut) : path;
}

const DOC_COMPARE_WIDTH_KEY = "marionette-doc-compare-width";
const DOC_COMPARE_MIN_DOC = 280;
const DOC_COMPARE_MIN_CHAT = 320;

/**
 * Side-by-side compare layout: conversation on the left, the active document
 * on the right, one draggable seam. Deliberately not the split-tree — doc
 * pages are pseudo-tabs, and this one-purpose row keeps the tree untouched.
 * The seam width persists; a press that never moved doesn't overwrite the
 * stored value (same lesson as the composer-height grip).
 */
export function DocCompareLayout({ children, doc }: { children: ReactNode; doc: ReactNode }) {
  const rowRef = useRef<HTMLDivElement>(null);
  const [docWidth, setDocWidth] = useState<number>(() => {
    const stored = Number(localStorage.getItem(DOC_COMPARE_WIDTH_KEY));
    return Number.isFinite(stored) && stored >= DOC_COMPARE_MIN_DOC ? stored : 0;
  });
  const draggingRef = useRef(false);
  const movedRef = useRef(false);

  const onSeamDown = (event: React.PointerEvent<HTMLDivElement>) => {
    draggingRef.current = true;
    movedRef.current = false;
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onSeamMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    const rect = rowRef.current?.getBoundingClientRect();
    if (!rect) return;
    movedRef.current = true;
    const width = Math.min(Math.max(rect.right - event.clientX, DOC_COMPARE_MIN_DOC), rect.width - DOC_COMPARE_MIN_CHAT);
    setDocWidth(width);
  };
  const onSeamUp = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    event.currentTarget.releasePointerCapture(event.pointerId);
    if (movedRef.current && docWidth > 0) {
      localStorage.setItem(DOC_COMPARE_WIDTH_KEY, String(Math.round(docWidth)));
    }
  };

  return (
    <div ref={rowRef} className="doc-compare-row">
      <div className="doc-compare__chat">{children}</div>
      <div
        className="doc-compare__seam"
        role="separator"
        aria-orientation="vertical"
        onPointerDown={onSeamDown}
        onPointerMove={onSeamMove}
        onPointerUp={onSeamUp}
        onPointerCancel={onSeamUp}
      />
      <div className="doc-compare__doc" style={docWidth ? { width: `${docWidth}px` } : undefined}>
        {doc}
      </div>
    </div>
  );
}

export function DocPage({
  path,
  onClose,
  onQuote,
  onNavigate,
  compareAvailable = false,
  compareActive = false,
  onToggleCompare,
  escToClose = true,
}: DocPageProps) {
  const t = useT();
  const [loaded, setLoaded] = useState<{ content: string; mtimeMs: number } | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [mode, setMode] = useState<"preview" | "edit">("preview");
  const [saving, setSaving] = useState(false);
  const [staleConfirm, setStaleConfirm] = useState(false);
  const [discardConfirm, setDiscardConfirm] = useState(false);

  const previewRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const [hasSelection, setHasSelection] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoadError(null);
    setLoaded(null);
    setDraft("");
    readDocFile(path, null)
      .then((file) => {
        if (cancelled) return;
        setLoaded({ content: file.content, mtimeMs: file.mtimeMs });
        setDraft(file.content);
      })
      .catch((error) => {
        if (!cancelled) setLoadError(error instanceof Error ? error.message : String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  const dir = parentDirOf(path);
  const dirty = loaded != null && draft !== loaded.content;

  // Preview-mode selection tracking, scoped to this page's rendered body.
  useEffect(() => {
    const onSelectionChange = () => {
      const selection = window.getSelection();
      if (!selection || selection.isCollapsed) {
        setHasSelection(false);
        return;
      }
      setHasSelection(Boolean(previewRef.current?.contains(selection.anchorNode)));
    };
    document.addEventListener("selectionchange", onSelectionChange);
    return () => document.removeEventListener("selectionchange", onSelectionChange);
  }, []);

  const doSave = useCallback(
    async (force = false) => {
      if (loaded == null || !dirty || saving) return;
      setSaving(true);
      try {
        const result = await saveDocFile(path, null, draft, loaded.mtimeMs, force);
        if (result.saved) {
          setLoaded({ content: draft, mtimeMs: result.mtimeMs });
          showToast({ text: t("doc.saved"), tone: "ok" });
        } else if (result.reason === "stale") {
          setStaleConfirm(true);
        }
      } catch (error) {
        showToast({
          text: t("doc.saveFailed"),
          detail: error instanceof Error ? error.message : String(error),
          tone: "error",
        });
      } finally {
        setSaving(false);
      }
    },
    [dirty, draft, loaded, path, saving, t],
  );

  const requestClose = useCallback(() => {
    if (dirty) {
      setDiscardConfirm(true);
      return;
    }
    onClose();
  }, [dirty, onClose]);

  // Ctrl+S saves from either mode; Esc closes (guarded for unsaved edits).
  // The doc page is only mounted while it is the active page, so these are
  // safe as window-level listeners.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void doSave();
        return;
      }
      if (event.key === "Escape" && escToClose) {
        requestClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [doSave, escToClose, requestClose]);

  // Relative `.md` links hop to another document tab (see DocNavContext).
  const navigateDoc = useCallback(
    (rawTarget: string) => {
      readDocFile(rawTarget, dir)
        .then((file) => onNavigate(file.path))
        .catch((error) => {
          showToast({
            text: t("doc.loadFailed"),
            detail: error instanceof Error ? error.message : String(error),
            tone: "error",
          });
        });
    },
    [dir, onNavigate, t],
  );

  const quoteToAgent = () => {
    let selected = "";
    if (mode === "edit" && editorRef.current) {
      const el = editorRef.current;
      selected = el.value.slice(el.selectionStart, el.selectionEnd);
    } else {
      const selection = window.getSelection();
      if (selection && !selection.isCollapsed && previewRef.current?.contains(selection.anchorNode)) {
        selected = selection.toString();
      }
    }
    const text = selected.trim();
    if (!text) {
      showToast({ text: t("doc.quoteEmpty"), tone: "error" });
      return;
    }
    const quoted = text
      .split("\n")
      .map((line) => `> ${line}`)
      .join("\n");
    onQuote(`${t("doc.quoteHeader", { path })}\n\n${quoted}`);
  };

  const openWithSystem = () => {
    void openExternal(path, null).catch(() => undefined);
  };

  if (loadError) {
    return (
      <div className="doc-page is-error">
        <div className="doc-page__bar">
          <FileText size={13} />
          <span className="doc-page__title" title={path}>
            <span className="doc-page__title-dir">{parentDirOf(path)}{" / "}</span>
            {fileNameOf(path)}
          </span>
          <div className="doc-page__actions">
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              title={t("doc.openExternal")}
              onClick={openWithSystem}
            >
              <ExternalLink size={12} />
            </button>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              title={t("doc.close")}
              onClick={onClose}
            >
              <X size={12} />
            </button>
          </div>
        </div>
        <div className="doc-page__error">{t("doc.loadFailed")}</div>
        <div className="doc-page__error-detail">{loadError}</div>
      </div>
    );
  }

  return (
    <div className={`doc-page${mode === "edit" ? " is-editing" : ""}`}>
      <div className="doc-page__bar">
        <FileText size={13} />
        <span className="doc-page__title" title={path}>
          <span className="doc-page__title-dir">{parentDirOf(path)}{" / "}</span>
          {fileNameOf(path)}
        </span>
        {dirty && (
          <i className="doc-page__dirty" title={t("doc.unsaved")} aria-label={t("doc.unsaved")} />
        )}
        <div className="doc-page__actions">
          <div className="doc-page__mode" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={mode === "preview"}
              className={`doc-page__mode-btn${mode === "preview" ? " is-active" : ""}`}
              onClick={() => setMode("preview")}
              title={t("doc.preview")}
            >
              <Eye size={12} />
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={mode === "edit"}
              className={`doc-page__mode-btn${mode === "edit" ? " is-active" : ""}`}
              onClick={() => setMode("edit")}
              title={t("doc.edit")}
            >
              <PencilLine size={12} />
            </button>
          </div>
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm"
            title={t("doc.saveHint")}
            disabled={!dirty || saving}
            onClick={() => void doSave()}
          >
            <Save size={12} />
          </button>
          {compareAvailable && onToggleCompare && (
            <button
              type="button"
              className={`pill-action pill-action--icon pill-action--sm${compareActive ? " is-active" : ""}`}
              title={compareActive ? t("doc.compareOff") : t("doc.compare")}
              onClick={onToggleCompare}
            >
              <Columns2 size={12} />
            </button>
          )}
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm"
            title={hasSelection ? t("doc.quoteToAgent") : t("doc.quoteHint")}
            // Keep the text selection alive — a plain click would collapse it
            // before the handler reads it.
            onMouseDown={(event) => event.preventDefault()}
            onClick={quoteToAgent}
          >
            <MessageSquarePlus size={12} />
          </button>
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm"
            title={t("doc.openExternal")}
            onClick={openWithSystem}
          >
            <ExternalLink size={12} />
          </button>
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm"
            title={t("doc.close")}
            onClick={requestClose}
          >
            <X size={12} />
          </button>
        </div>
      </div>
      <div className="doc-page__body">
        {mode === "preview" ? (
          <div ref={previewRef} className="doc-page__preview">
            <MarkdownImageCwdContext.Provider value={dir}>
              <LinkCwdContext.Provider value={dir}>
                <DocNavContext.Provider value={navigateDoc}>
                  {loaded ? <MarkdownBody text={draft} /> : null}
                </DocNavContext.Provider>
              </LinkCwdContext.Provider>
            </MarkdownImageCwdContext.Provider>
          </div>
        ) : (
          <textarea
            ref={editorRef}
            className="doc-page__editor"
            value={draft}
            spellCheck={false}
            onChange={(event) => setDraft(event.target.value)}
            disabled={loaded == null}
          />
        )}
      </div>
      {staleConfirm && (
        <ConfirmDialog
          title={t("doc.staleTitle")}
          itemName={fileNameOf(path)}
          description={t("doc.staleBody")}
          confirmLabel={t("doc.overwrite")}
          danger={false}
          onConfirm={() => {
            setStaleConfirm(false);
            void doSave(true);
          }}
          onCancel={() => setStaleConfirm(false)}
        />
      )}
      {discardConfirm && (
        <ConfirmDialog
          title={t("doc.discardTitle")}
          itemName={fileNameOf(path)}
          description={t("doc.discardBody")}
          confirmLabel={t("doc.discard")}
          onConfirm={() => {
            setDiscardConfirm(false);
            onClose();
          }}
          onCancel={() => setDiscardConfirm(false)}
        />
      )}
    </div>
  );
}
