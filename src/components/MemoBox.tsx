/**
 * Memo inbox page (edge dock stage 3, docs/plans/plan-edge-dock.md §4 / §4.1).
 *
 * Mounted either as the dock strip's full page or as the floating window's
 * titlebar tab page — the mount's chrome owns title and close, Esc still
 * closes.
 *
 * 收 — Explorer drag-in (whole window while this page is active), text/link
 *      OLE drag (wry patch), Ctrl+V, and the write card at the top.
 * 找 — search + kind filters + tag chips (2026-11-18) + day-grouped post
 *      cards; pinned entries float in their own top group.
 * 用 — drag ANY card onto the composer (file/image → attachment, text/link
 *      → body at the caret), or pick several (选择 / Ctrl-click) and hand
 *      them to a new dialog.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  Check,
  FileText,
  Image as ImageIcon,
  Link2,
  MessageSquarePlus,
  Pencil,
  Pin,
  PinOff,
  Search,
  Tag,
  Trash2,
  Type,
  X,
} from "lucide-react";
import { isTauriRuntime, openExternal } from "../lib/api";
import { isDocPath, openInDocTabIfEligible } from "../lib/openFile";
import { isPageDragSource, onDragText } from "../lib/dragText";
import {
  encodeMemoDrag,
  filterMemoEntries,
  formatMemoAge,
  formatMemoSize,
  MEMO_DRAG_TYPE,
  memoDelete,
  memoDragText,
  memoImageOrient,
  memoLinkHost,
  memoList,
  memoSavePaths,
  memoSaveText,
  memoTagVocabulary,
  memoUpdate,
  splitPinnedGroup,
  type MemoEntry,
  type MemoKind,
  type MemoPatch,
} from "../lib/memos";
import { showToast } from "../lib/toast";
import { useT, type DictKey } from "../lib/i18n";

const FILTERS: ReadonlyArray<{ id: MemoKind | "all"; label: DictKey }> = [
  { id: "all", label: "memo.kindAll" },
  { id: "link", label: "memo.kindLink" },
  { id: "text", label: "memo.kindText" },
  { id: "image", label: "memo.kindImage" },
  { id: "file", label: "memo.kindFile" },
];

const KIND_LABELS: Record<MemoKind, DictKey> = {
  link: "memo.kindLink",
  text: "memo.kindText",
  image: "memo.kindImage",
  file: "memo.kindFile",
};

/** Tag filter chips before the "+n" overflow toggle. */
const TAG_FILTER_LIMIT = 10;

function kindIcon(kind: MemoKind) {
  if (kind === "link") return <Link2 size={14} />;
  if (kind === "image") return <ImageIcon size={14} />;
  if (kind === "text") return <Type size={14} />;
  return <FileText size={14} />;
}

function memoFileSrc(path: string): string {
  if (isTauriRuntime()) return convertFileSrc(path);
  return `file:///${path.replace(/\\/g, "/")}`;
}

function fileMeta(entry: MemoEntry, kindLabel: string): string {
  return [kindLabel, formatMemoSize(entry.size), entry.source].filter(Boolean).join(" · ");
}

type Props = {
  onClose: () => void;
  onChanged: (count: number) => void;
  /** Hand the picked entries to a new dialog (prefilled, not sent). */
  onUse: (entries: MemoEntry[], intent: "dialog" | "doc") => void;
  /** Adjacent-to-chat mount: drags and Esc belong to this pane only. */
  embedded?: boolean;
  /** Bumped by the App on ANY memo write elsewhere (composer save-to-box); re-lists. */
  revision?: number;
  /** Detached surface shell: no conversations to start — hide the selbar use actions. */
  hideUseActions?: boolean;
};

export function MemoBox({ onClose, onChanged, onUse, embedded = false, revision = 0, hideUseActions = false }: Props) {
  const t = useT();
  const [entries, setEntries] = useState<MemoEntry[]>([]);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<MemoKind | "all">("all");
  const [activeTags, setActiveTags] = useState<string[]>([]);
  const [tagsExpanded, setTagsExpanded] = useState(false);
  const [dropActive, setDropActive] = useState(false);
  const [busy, setBusy] = useState(false);
  const [selecting, setSelecting] = useState(false);
  const [picked, setPicked] = useState<ReadonlySet<string>>(() => new Set());
  const [draft, setDraft] = useState("");
  const [openTextId, setOpenTextId] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const composeRef = useRef<HTMLTextAreaElement>(null);
  const selectingRef = useRef(selecting);
  selectingRef.current = selecting;

  const publish = useCallback(
    (next: MemoEntry[]) => {
      setEntries(next);
      onChanged(next.length);
      setPicked((prev) => {
        if (prev.size === 0) return prev;
        const alive = new Set(next.map((entry) => entry.id));
        return new Set([...prev].filter((id) => alive.has(id)));
      });
    },
    [onChanged],
  );

  useEffect(() => {
    let disposed = false;
    void memoList().then((rows) => {
      if (!disposed) publish(rows);
    });
    return () => {
      disposed = true;
    };
  }, [publish, revision]);

  useEffect(() => {
    composeRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (
        embedded &&
        event.target instanceof Node &&
        rootRef.current &&
        !rootRef.current.contains(event.target)
      ) {
        return;
      }
      if (selectingRef.current) {
        setSelecting(false);
        setPicked(new Set());
      } else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [embedded, onClose]);

  // Explorer drag-in. Standalone / dock memo pages own the whole window; the
  // embedded workspace mount is rect-scoped so a drop over the Agent chat
  // still belongs to Composer. Armed only by enter with OS paths: tao also
  // pings for in-page row drags, which must stay on the pure HTML5 channel.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let armed = false;
    const insideMemo = (position: { x: number; y: number } | null): boolean => {
      if (!position || !embedded) return !embedded;
      const rect = rootRef.current?.getBoundingClientRect();
      if (!rect) return false;
      const scale = window.devicePixelRatio || 1;
      const x = position.x / scale;
      const y = position.y / scale;
      return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
    };
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (disposed) return;
        const payload = event.payload;
        if (payload.type === "leave") {
          armed = false;
          setDropActive(false);
          return;
        }
        if (payload.type === "enter") {
          // Arm on the paths alone: a drag from Explorer enters the window at
          // its edge, usually over the chat half. Position only gates the
          // highlight and the final drop, never whether the drag is ours.
          armed = Boolean(payload.paths?.length);
          setDropActive(armed && insideMemo(payload.position));
          return;
        }
        if (payload.type === "over") {
          setDropActive(armed && insideMemo(payload.position));
          return;
        }
        if (!armed) return;
        if (payload.type === "drop") {
          armed = false;
          setDropActive(false);
          const paths = payload.paths ?? [];
          if (!paths.length || !insideMemo(payload.position)) return;
          setBusy(true);
          void memoSavePaths(paths, t("memo.sourceDrop"))
            .then(publish)
            .catch((error) => showToast({ text: t("memo.saveFailed"), detail: String(error), tone: "error" }))
            .finally(() => setBusy(false));
        }
      })
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [embedded, publish]);

  // Text/link drag-in (browser selections, URLs) — the wry-patch channel.
  // Standalone owns the window; embedded is scoped to the memo rectangle.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const insideMemo = (position: { x: number; y: number }): boolean => {
      if (!embedded) return true;
      const rect = rootRef.current?.getBoundingClientRect();
      if (!rect) return false;
      const scale = window.devicePixelRatio || 1;
      const x = position.x / scale;
      const y = position.y / scale;
      return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
    };
    void onDragText((event) => {
      if (disposed || isPageDragSource()) return;
      const over = insideMemo({ x: event.x, y: event.y });
      if (event.kind === "enter" || event.kind === "over") {
        setDropActive(over);
        return;
      }
      setDropActive(false);
      if (event.kind !== "drop" || !over) return;
      const text = event.text.trim();
      if (!text) return;
      setBusy(true);
      void memoSaveText(text, undefined, t("memo.sourceDrop"))
        .then((saved) => {
          if (!saved) {
            showToast({ text: t("memo.saveFailed"), tone: "error" });
            return;
          }
          return memoList().then(publish);
        })
        .finally(() => setBusy(false));
    })
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [embedded, publish]);

  const saveText = useCallback(
    (text: string, source: string) => {
      const trimmed = text.trim();
      if (!trimmed) return Promise.resolve(false);
      setBusy(true);
      return memoSaveText(trimmed, undefined, source)
        .then((saved) => {
          if (!saved) {
            showToast({ text: t("memo.saveFailed"), tone: "error" });
            return false;
          }
          return memoList().then((rows) => {
            publish(rows);
            return true;
          });
        })
        .finally(() => setBusy(false));
    },
    [publish],
  );

  const onPaste = useCallback(
    (event: React.ClipboardEvent) => {
      // Search and the write card keep their own paste.
      const tag = (event.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      const text = event.clipboardData.getData("text/plain").trim();
      if (!text) return;
      event.preventDefault();
      void saveText(text, t("memo.sourcePaste"));
    },
    [saveText],
  );

  const growCompose = (el: HTMLTextAreaElement) => {
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  };

  const submitCompose = () => {
    const text = draft;
    void saveText(text, t("memo.sourceWrite")).then((ok) => {
      if (!ok) return;
      setDraft("");
      const el = composeRef.current;
      if (el) {
        el.style.height = "";
        el.focus();
      }
    });
  };

  const filtered = useMemo(
    () => filterMemoEntries(entries, { kind: filter, query, activeTags }),
    [entries, filter, query, activeTags],
  );

  const { pinned, groups } = useMemo(() => splitPinnedGroup(filtered), [filtered]);

  const vocabulary = useMemo(() => memoTagVocabulary(entries), [entries]);

  const toggleTagFilter = (tag: string) => {
    setActiveTags((prev) => (prev.includes(tag) ? prev.filter((x) => x !== tag) : [...prev, tag]));
  };

  const counts = useMemo(() => {
    const map = new Map<MemoKind | "all", number>([["all", entries.length]]);
    for (const entry of entries) map.set(entry.kind, (map.get(entry.kind) ?? 0) + 1);
    return map;
  }, [entries]);

  const remove = (id: string) => {
    void memoDelete(id).then((next) => {
      if (next) publish(next);
      else showToast({ text: t("memo.deleteFailed"), tone: "error" });
    });
  };

  const update = (id: string, patch: MemoPatch) => {
    void memoUpdate(id, patch).then((next) => {
      if (next) publish(next);
      else showToast({ text: t("memo.updateFailed"), tone: "error" });
    });
  };

  const toggle = (id: string) => {
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const exitSelect = () => {
    setSelecting(false);
    setPicked(new Set());
  };

  const use = (intent: "dialog" | "doc") => {
    const chosen = entries.filter((entry) => picked.has(entry.id));
    if (!chosen.length) return;
    onUse(chosen, intent);
    onClose();
  };

  const openTarget = (target: string) => {
    // Memo file paths are absolute; markdown/text opens in the doc tab, links
    // and other kinds fall through to the OS handler.
    if (isDocPath(target) && openInDocTabIfEligible(target)) return;
    void openExternal(target)
      .then((result) => {
        if (!result?.opened) {
          showToast({ text: result?.message ?? t("memo.openFailed"), tone: "error" });
        }
      })
      .catch((error) => showToast({ text: t("memo.openFailed"), detail: String(error), tone: "error" }));
  };

  const onCardClick = (event: React.MouseEvent, entry: MemoEntry) => {
    if (selecting) {
      toggle(entry.id);
      return;
    }
    if (event.ctrlKey || event.metaKey) {
      setSelecting(true);
      toggle(entry.id);
      return;
    }
    if (entry.kind === "text") {
      setOpenTextId((current) => (current === entry.id ? null : entry.id));
      return;
    }
    if (entry.kind === "link") {
      const url = entry.content?.trim();
      if (url) openTarget(url);
      return;
    }
    if (entry.path) openTarget(entry.path);
  };

  const visibleVocabulary = tagsExpanded ? vocabulary : vocabulary.slice(0, TAG_FILTER_LIMIT);
  const hiddenTagCount = Math.max(0, vocabulary.length - TAG_FILTER_LIMIT);

  return (
    <div
      ref={rootRef}
      className={"memo-box" + (embedded ? " is-embedded" : "") + (dropActive ? " is-drop-active" : "") + (selecting ? " is-selecting" : "")}
      role="dialog"
      aria-label={t("dock.pageMemo")}
      tabIndex={-1}
      onPaste={onPaste}
    >
      {/* Title / close live in the mount's chrome (dock strip title bar or the
          titlebar tab); Esc and the strip's 返回 still close. */}
      <div className="memo-box__compose">
        <textarea
          ref={composeRef}
          rows={2}
          placeholder={t("v2memo.compose")}
          aria-label={t("memo.composeAria")}
          value={draft}
          disabled={busy}
          onChange={(event) => {
            setDraft(event.target.value);
            growCompose(event.target);
          }}
          onKeyDown={(event) => {
            if (!(event.ctrlKey || event.metaKey) || event.key !== "Enter") return;
            if (event.nativeEvent.isComposing) return;
            event.preventDefault();
            submitCompose();
          }}
        />
        <div className="memo-box__compose-bar">
          <span className="memo-box__compose-hint">{t("v2memo.composeHint")}</span>
          <button
            type="button"
            className="memo-box__compose-save"
            disabled={busy || !draft.trim()}
            onClick={submitCompose}
          >
            {t("common.save")}
          </button>
        </div>
      </div>
      <div className="memo-box__head">
        <label className="memo-box__search">
          <Search size={14} />
          <input
            type="search"
            placeholder={t("memo.searchPlaceholder")}
            aria-label={t("memo.searchAria")}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          {busy && <span className="memo-box__busy">{t("memo.saving")}</span>}
        </label>
        {entries.length > 0 && (
          <button
            type="button"
            className={selecting ? "memo-box__text-btn is-on" : "memo-box__text-btn"}
            onClick={() => (selecting ? exitSelect() : setSelecting(true))}
          >
            {selecting ? t("memo.done") : t("memo.select")}
          </button>
        )}
      </div>
      {entries.length > 0 && (
        <div className="memo-box__filters">
          {FILTERS.filter((chip) => chip.id === "all" || (counts.get(chip.id) ?? 0) > 0).map((chip) => (
            <button
              key={chip.id}
              type="button"
              className={filter === chip.id ? "memo-box__chip is-on" : "memo-box__chip"}
              onClick={() => setFilter(chip.id)}
            >
              {t(chip.label)}
              <b>{counts.get(chip.id) ?? 0}</b>
            </button>
          ))}
        </div>
      )}
      {vocabulary.length > 0 && (
        <div className="memo-box__filters memo-box__tagfilter">
          {visibleVocabulary.map(({ tag, count }) => (
            <button
              key={tag}
              type="button"
              className={activeTags.includes(tag) ? "memo-box__chip is-on" : "memo-box__chip"}
              aria-pressed={activeTags.includes(tag)}
              onClick={() => toggleTagFilter(tag)}
            >
              #{tag}
              <b>{count}</b>
            </button>
          ))}
          {hiddenTagCount > 0 && (
            <button
              type="button"
              className="memo-box__chip memo-box__chip--more"
              aria-label={t("memo.moreTags")}
              title={t("memo.moreTags")}
              onClick={() => setTagsExpanded((v) => !v)}
            >
              {tagsExpanded ? `−${hiddenTagCount}` : `+${hiddenTagCount}`}
            </button>
          )}
        </div>
      )}
      <div className="memo-box__list custom-scrollbar">
        {entries.length === 0 ? (
          <div className="memo-box__empty">
            <strong>{t("memo.emptyTitle")}</strong>
            <span>{t("memo.emptyHint")}</span>
            <span>{t("memo.emptyHint2")}</span>
          </div>
        ) : filtered.length === 0 ? (
          <p className="memo-box__none">{t("memo.noMatches")}</p>
        ) : (
          <>
            {pinned.length > 0 && (
              <section className="memo-box__group memo-box__group--pinned">
                <h3 className="memo-box__day">
                  <Pin size={12} aria-hidden />
                  {t("memo.pinnedGroup")}
                </h3>
                <div className="memo-box__feed">
                  {pinned.map((entry) => (
                    <MemoCard
                      key={entry.id}
                      entry={entry}
                      selecting={selecting}
                      picked={picked.has(entry.id)}
                      textOpen={openTextId === entry.id}
                      onClick={(event) => onCardClick(event, entry)}
                      onToggle={() => {
                        if (!selecting) setSelecting(true);
                        toggle(entry.id);
                      }}
                      onDelete={() => remove(entry.id)}
                      onPatch={(patch) => update(entry.id, patch)}
                      onTagClick={toggleTagFilter}
                      activeTags={activeTags}
                    />
                  ))}
                </div>
              </section>
            )}
            {groups.map((group) => (
              <section key={group.key} className="memo-box__group">
                {group.label && <h3 className="memo-box__day">{group.label}</h3>}
                <div className="memo-box__feed">
                  {group.entries.map((entry) => (
                    <MemoCard
                      key={entry.id}
                      entry={entry}
                      selecting={selecting}
                      picked={picked.has(entry.id)}
                      textOpen={openTextId === entry.id}
                      onClick={(event) => onCardClick(event, entry)}
                      onToggle={() => {
                        if (!selecting) setSelecting(true);
                        toggle(entry.id);
                      }}
                      onDelete={() => remove(entry.id)}
                      onPatch={(patch) => update(entry.id, patch)}
                      onTagClick={toggleTagFilter}
                      activeTags={activeTags}
                    />
                  ))}
                </div>
              </section>
            ))}
          </>
        )}
      </div>
      {selecting ? (
        <div className="memo-box__selbar">
          <span>{t("v2memo.selected", { n: picked.size })}</span>
          <span className="memo-box__sel-hint">{t("v2memo.selHint")}</span>
          <span className="memo-box__spacer" />
          <button type="button" className="memo-box__act memo-box__act--ghost" onClick={exitSelect}>
            {t("v2memo.cancelSelect")}
          </button>
          {!hideUseActions && (
            <>
              <button type="button" className="memo-box__act" disabled={!picked.size} onClick={() => use("doc")}>
                <FileText size={14} />
                {t("memo.makeDoc")}
              </button>
              <button type="button" className="memo-box__act is-primary" disabled={!picked.size} onClick={() => use("dialog")}>
                <MessageSquarePlus size={14} />
                {t("memo.newDialog")}
              </button>
            </>
          )}
        </div>
      ) : (
        <div className={dropActive ? "memo-box__drop is-active" : "memo-box__drop"}>
          {dropActive ? t("memo.dropRelease") : t("memo.dropHint")}
        </div>
      )}
    </div>
  );
}

type CardProps = {
  entry: MemoEntry;
  selecting: boolean;
  picked: boolean;
  textOpen: boolean;
  onClick: (event: React.MouseEvent) => void;
  onToggle: () => void;
  onDelete: () => void;
  /** Apply an in-place patch (pin / edit / tags). */
  onPatch: (patch: MemoPatch) => void;
  /** Clicking a tag chip on a card toggles it in the feed filter. */
  onTagClick: (tag: string) => void;
  activeTags: readonly string[];
};

function applyShotOrient(img: HTMLImageElement) {
  if (!img.naturalWidth || !img.naturalHeight) return;
  img.dataset.orient = memoImageOrient(img.naturalWidth, img.naturalHeight);
}

function MemoCard({ entry, selecting, picked, textOpen, onClick, onToggle, onDelete, onPatch, onTagClick, activeTags }: CardProps) {
  const t = useT();
  const [imageBroken, setImageBroken] = useState(false);
  // Edit / tag editors are card-local: the component is keyed by entry.id, so
  // publish() re-renders do not blow away a form mid-edit.
  const [editing, setEditing] = useState(false);
  const [titleDraft, setTitleDraft] = useState(entry.title);
  const [contentDraft, setContentDraft] = useState(entry.content ?? "");
  const [tagsOpen, setTagsOpen] = useState(false);
  const [tagDraft, setTagDraft] = useState("");
  const contentEditable = entry.kind === "text" || entry.kind === "link";
  const tags = entry.tags ?? [];
  const draggable = !selecting && !editing && !tagsOpen;
  const kindClass =
    entry.kind === "image" && !imageBroken
      ? "memo-box__card--image"
      : entry.kind === "text"
        ? "memo-box__card--text"
        : entry.kind === "link"
          ? "memo-box__card--link"
          : "memo-box__card--file";
  const className = [
    "memo-box__card",
    kindClass,
    picked ? "is-picked" : "",
    entry.pinned ? "is-pinned" : "",
    entry.kind === "text" && textOpen ? "is-open" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const host = entry.kind === "link" ? memoLinkHost(entry.content) : "";
  const url = entry.kind === "link" ? (entry.content ?? "") : "";
  const titleIsUrl = Boolean(url) && (entry.title === url || url.startsWith(entry.title));

  const startEdit = () => {
    setTitleDraft(entry.title);
    setContentDraft(entry.content ?? "");
    setTagsOpen(false);
    setEditing(true);
  };

  const saveEdit = () => {
    const patch: MemoPatch = {};
    const title = titleDraft.trim();
    const content = contentDraft.trim();
    if (title && title !== entry.title) patch.title = title;
    if (contentEditable && content && content !== entry.content) patch.content = content;
    setEditing(false);
    if (patch.title !== undefined || patch.content !== undefined) onPatch(patch);
  };

  const addTag = () => {
    const tag = tagDraft.trim();
    setTagDraft("");
    if (!tag || tags.includes(tag)) return;
    onPatch({ tags: [...tags, tag] });
  };

  const removeTag = (tag: string) => {
    onPatch({ tags: tags.filter((x) => x !== tag) });
  };

  const editorKeyDown = (event: React.KeyboardEvent) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    setEditing(false);
    setTagsOpen(false);
  };

  return (
    <article
      className={className}
      draggable={draggable}
      onDragStart={(event) => {
        event.dataTransfer.setData(MEMO_DRAG_TYPE, encodeMemoDrag(entry));
        event.dataTransfer.setData("text/plain", memoDragText(entry));
        event.dataTransfer.effectAllowed = "copy";
      }}
      onClick={onClick}
      title={
        entry.kind === "text"
          ? textOpen
            ? t("memo.clickCollapse")
            : t("memo.clickExpand")
          : entry.kind === "link"
            ? url
            : `${entry.path ?? entry.title}${draggable ? `\n${t("memo.dragHint")}` : ""}`
      }
    >
      <button
        type="button"
        className={picked ? "memo-box__check is-on" : "memo-box__check"}
        aria-label={picked ? t("memo.unpickAria", { title: entry.title }) : t("memo.pickAria", { title: entry.title })}
        aria-pressed={picked}
        onClick={(event) => {
          event.stopPropagation();
          onToggle();
        }}
      >
        {picked && <Check size={10} strokeWidth={3} />}
      </button>
      {editing ? (
        <div className="memo-box__edit" onClick={(event) => event.stopPropagation()}>
          <input
            className="memo-box__edit-title"
            value={titleDraft}
            placeholder={t("memo.editTitle")}
            aria-label={t("memo.editTitle")}
            onChange={(event) => setTitleDraft(event.target.value)}
            onKeyDown={(event) => {
              editorKeyDown(event);
              if (event.key === "Enter") {
                event.preventDefault();
                saveEdit();
              }
            }}
          />
          {contentEditable && (
            <textarea
              className="memo-box__edit-content"
              rows={4}
              value={contentDraft}
              placeholder={t("memo.editContent")}
              aria-label={t("memo.editContent")}
              onChange={(event) => setContentDraft(event.target.value)}
              onKeyDown={(event) => {
                editorKeyDown(event);
                if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
                  event.preventDefault();
                  saveEdit();
                }
              }}
            />
          )}
          <div className="memo-box__edit-bar">
            <button type="button" className="memo-box__act memo-box__act--ghost" onClick={() => setEditing(false)}>
              {t("common.cancel")}
            </button>
            <button type="button" className="memo-box__act is-primary" onClick={saveEdit}>
              {t("common.save")}
            </button>
          </div>
        </div>
      ) : entry.kind === "image" && entry.path && !imageBroken ? (
        <>
          <div className="memo-box__shot-wrap">
            <img
              className="memo-box__shot"
              src={memoFileSrc(entry.path)}
              alt={entry.title}
              draggable={false}
              decoding="async"
              ref={(img) => {
                if (img?.complete) applyShotOrient(img);
              }}
              onLoad={(event) => applyShotOrient(event.currentTarget)}
              onError={() => setImageBroken(true)}
            />
          </div>
          <div className="memo-box__card-body">
            <span className="memo-box__title">{entry.title}</span>
            <CardFoot
              entry={entry}
              selecting={selecting}
              onDelete={onDelete}
              onPatch={onPatch}
              onEdit={startEdit}
              onToggleTags={() => setTagsOpen((v) => !v)}
              onTagClick={onTagClick}
              activeTags={activeTags}
            />
          </div>
        </>
      ) : entry.kind === "text" ? (
        <>
          <p className="memo-box__note">{entry.content || entry.title}</p>
          <CardFoot
            entry={entry}
            selecting={selecting}
            onDelete={onDelete}
            onPatch={onPatch}
            onEdit={startEdit}
            onToggleTags={() => setTagsOpen((v) => !v)}
            onTagClick={onTagClick}
            activeTags={activeTags}
          />
        </>
      ) : entry.kind === "link" ? (
        <>
          <div className="memo-box__link-kicker">
            <span className="memo-box__icon" aria-hidden>
              {kindIcon("link")}
            </span>
            <span className="memo-box__link-host">{host || t("memo.kindLink")}</span>
          </div>
          {!titleIsUrl && entry.title && <span className="memo-box__title">{entry.title}</span>}
          {url && <span className="memo-box__url">{url}</span>}
          <CardFoot
            entry={entry}
            selecting={selecting}
            onDelete={onDelete}
            onPatch={onPatch}
            onEdit={startEdit}
            onToggleTags={() => setTagsOpen((v) => !v)}
            onTagClick={onTagClick}
            activeTags={activeTags}
          />
        </>
      ) : (
        <>
          <div className="memo-box__file">
            <span className="memo-box__icon" aria-hidden>
              {kindIcon(entry.kind)}
            </span>
            <span className="memo-box__body">
              <span className="memo-box__title">{entry.title}</span>
              {fileMeta(entry, t("memo.kindFile")) && <span className="memo-box__meta">{fileMeta(entry, t("memo.kindFile"))}</span>}
            </span>
          </div>
          <CardFoot
            entry={entry}
            selecting={selecting}
            onDelete={onDelete}
            onPatch={onPatch}
            onEdit={startEdit}
            onToggleTags={() => setTagsOpen((v) => !v)}
            onTagClick={onTagClick}
            activeTags={activeTags}
          />
        </>
      )}
      {tagsOpen && (
        <div className="memo-box__tag-editor" onClick={(event) => event.stopPropagation()}>
          {tags.length === 0 && <span className="memo-box__tag-empty">{t("memo.noTags")}</span>}
          {tags.map((tag) => (
            <span key={tag} className={activeTags.includes(tag) ? "memo-box__tag is-on" : "memo-box__tag"}>
              <button type="button" className="memo-box__tag-label" onClick={() => onTagClick(tag)}>
                #{tag}
              </button>
              <button
                type="button"
                className="memo-box__tag-x"
                aria-label={t("memo.tagRemoveAria", { tag })}
                onClick={() => removeTag(tag)}
              >
                <X size={10} />
              </button>
            </span>
          ))}
          <input
            className="memo-box__tag-input"
            value={tagDraft}
            placeholder={t("memo.addTag")}
            aria-label={t("memo.addTag")}
            onChange={(event) => setTagDraft(event.target.value)}
            onKeyDown={(event) => {
              editorKeyDown(event);
              if (event.key === "Enter") {
                event.preventDefault();
                addTag();
              }
            }}
            onBlur={addTag}
          />
        </div>
      )}
    </article>
  );
}

function CardFoot({
  entry,
  selecting,
  onDelete,
  onPatch,
  onEdit,
  onToggleTags,
  onTagClick,
  activeTags,
}: {
  entry: MemoEntry;
  selecting: boolean;
  onDelete: () => void;
  onPatch: (patch: MemoPatch) => void;
  onEdit: () => void;
  onToggleTags: () => void;
  onTagClick: (tag: string) => void;
  activeTags: readonly string[];
}) {
  const t = useT();
  const pinned = entry.pinned === true;
  const tags = entry.tags ?? [];
  return (
    <div className="memo-box__foot">
      {entry.kind !== "file" && <span className="memo-box__meta">{t(KIND_LABELS[entry.kind])}</span>}
      {tags.length > 0 && (
        <span className="memo-box__tags">
          {tags.slice(0, 4).map((tag) => (
            <button
              key={tag}
              type="button"
              className={activeTags.includes(tag) ? "memo-box__tag is-on" : "memo-box__tag"}
              onClick={(event) => {
                event.stopPropagation();
                onTagClick(tag);
              }}
            >
              #{tag}
            </button>
          ))}
          {tags.length > 4 && <span className="memo-box__tag-more">+{tags.length - 4}</span>}
        </span>
      )}
      <span className="memo-box__age">{formatMemoAge(entry.createdAt)}</span>
      {!selecting && (
        <span className="memo-box__cardacts">
          <button
            type="button"
            className={pinned ? "memo-box__cardact is-on" : "memo-box__cardact"}
            aria-label={pinned ? t("memo.unpin", { title: entry.title }) : t("memo.pin", { title: entry.title })}
            aria-pressed={pinned}
            title={pinned ? t("memo.unpin", { title: entry.title }) : t("memo.pin", { title: entry.title })}
            onClick={(event) => {
              event.stopPropagation();
              onPatch({ pinned: !pinned });
            }}
            onMouseDown={(event) => event.stopPropagation()}
          >
            {pinned ? <PinOff size={13} /> : <Pin size={13} />}
          </button>
          <button
            type="button"
            className="memo-box__cardact"
            aria-label={t("memo.edit", { title: entry.title })}
            title={t("memo.edit", { title: entry.title })}
            onClick={(event) => {
              event.stopPropagation();
              onEdit();
            }}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <Pencil size={13} />
          </button>
          <button
            type="button"
            className="memo-box__cardact"
            aria-label={t("memo.tagButton", { title: entry.title })}
            title={t("memo.tagButton", { title: entry.title })}
            onClick={(event) => {
              event.stopPropagation();
              onToggleTags();
            }}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <Tag size={13} />
          </button>
          <button
            type="button"
            className="memo-box__delete"
            aria-label={t("memo.deleteAria", { title: entry.title })}
            title={t("common.delete")}
            onClick={(event) => {
              event.stopPropagation();
              onDelete();
            }}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <Trash2 size={14} />
          </button>
        </span>
      )}
    </div>
  );
}
