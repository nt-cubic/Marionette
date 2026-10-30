/**
 * Memo inbox page (edge dock stage 3, docs/plan-edge-dock.md §4 / §4.1).
 *
 * Mounted either as the dock strip's full page or as the floating window's
 * titlebar tab page — the mount's chrome owns title and close, Esc still
 * closes.
 *
 * 收 — Explorer drag-in (whole window while this page is active), text/link
 *      OLE drag (wry patch), Ctrl+V, and the write card at the top.
 * 找 — search + kind filters + day-grouped post cards.
 * 用 — drag a file/image card onto the composer, or pick several (选择 /
 *      Ctrl-click) and hand them to a new dialog.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Check, File as FileIcon, FileText, Image as ImageIcon, Link2, MessageSquarePlus, Search, Trash2, Type } from "lucide-react";
import { isTauriRuntime, openExternal } from "../lib/api";
import { isPageDragSource, onDragText } from "../lib/dragText";
import {
  formatMemoAge,
  formatMemoSize,
  groupMemosByDay,
  MEMO_DRAG_TYPE,
  memoDelete,
  memoImageOrient,
  memoLinkHost,
  memoList,
  memoSavePaths,
  memoSaveText,
  type MemoEntry,
  type MemoKind,
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

function kindIcon(kind: MemoKind) {
  if (kind === "link") return <Link2 size={14} />;
  if (kind === "image") return <ImageIcon size={14} />;
  if (kind === "text") return <Type size={14} />;
  return <FileIcon size={14} />;
}

function memoFileSrc(path: string): string {
  if (isTauriRuntime()) return convertFileSrc(path);
  return `file:///${path.replace(/\\/g, "/")}`;
}

function fileMeta(entry: MemoEntry): string {
  return [formatMemoSize(entry.size), entry.source].filter(Boolean).join(" · ");
}

type Props = {
  onClose: () => void;
  onChanged: (count: number) => void;
  /** Hand the picked entries to a new dialog (prefilled, not sent). */
  onUse: (entries: MemoEntry[], intent: "dialog" | "doc") => void;
};

export function MemoBox({ onClose, onChanged, onUse }: Props) {
  const t = useT();
  const [entries, setEntries] = useState<MemoEntry[]>([]);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<MemoKind | "all">("all");
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
  }, [publish]);

  useEffect(() => {
    composeRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (selectingRef.current) {
        setSelecting(false);
        setPicked(new Set());
      } else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Explorer drag-in. The memo page owns the WHOLE window while it is the
  // active view: no conversation is in sight, so nothing else competes for
  // files, and a rect-precise hit test made drops that landed on the shelf /
  // context panel / tab strip die silently (2026-10-26 — 收纳盒 had never
  // accepted a single drop in practice). Armed only by an `enter` that
  // carries OS paths: tao also pings for in-page memo row drags, which carry
  // none and must stay pure HTML5. While docked, App's DockDropRouter stands
  // down entirely for this page.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let armed = false;
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
          armed = Boolean(payload.paths?.length);
          setDropActive(armed);
          return;
        }
        if (!armed) return;
        if (payload.type === "drop") {
          armed = false;
          setDropActive(false);
          const paths = payload.paths ?? [];
          if (!paths.length) return;
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
  }, [publish]);

  // Text/link drag-in (browser selections, URLs) — the wry-patch channel.
  // Same whole-window ownership as files above; Rust derives the title and
  // turns bare URLs into link entries.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onDragText((event) => {
      if (disposed || isPageDragSource()) return;
      if (event.kind === "enter" || event.kind === "over") {
        setDropActive(true);
        return;
      }
      setDropActive(false);
      if (event.kind !== "drop") return;
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
  }, [publish]);

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

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return entries.filter((entry) => {
      if (filter !== "all" && entry.kind !== filter) return false;
      if (!q) return true;
      const haystack = [entry.title, entry.content ?? "", entry.path ?? ""].join("\n").toLowerCase();
      return haystack.includes(q);
    });
  }, [entries, filter, query]);

  const groups = useMemo(() => groupMemosByDay(filtered), [filtered]);

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

  return (
    <div
      ref={rootRef}
      className={`memo-box${dropActive ? " is-drop-active" : ""}${selecting ? " is-selecting" : ""}`}
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
          placeholder={t("memo.composePlaceholder")}
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
          <span>Ctrl + Enter</span>
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
          <Search size={12} />
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
          groups.map((group) => (
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
                    onToggle={() => toggle(entry.id)}
                    onDelete={() => remove(entry.id)}
                  />
                ))}
              </div>
            </section>
          ))
        )}
      </div>
      {selecting ? (
        <div className="memo-box__selbar">
          <span>
            {t("memo.picked")} <b>{picked.size}</b>
          </span>
          <span className="memo-box__spacer" />
          <button type="button" className="memo-box__act" disabled={!picked.size} onClick={() => use("doc")}>
            <FileText size={12} />
            {t("memo.makeDoc")}
          </button>
          <button type="button" className="memo-box__act is-primary" disabled={!picked.size} onClick={() => use("dialog")}>
            <MessageSquarePlus size={12} />
            {t("memo.newDialog")}
          </button>
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
};

function applyShotOrient(img: HTMLImageElement) {
  if (!img.naturalWidth || !img.naturalHeight) return;
  img.dataset.orient = memoImageOrient(img.naturalWidth, img.naturalHeight);
}

function MemoCard({ entry, selecting, picked, textOpen, onClick, onToggle, onDelete }: CardProps) {
  const t = useT();
  const [imageBroken, setImageBroken] = useState(false);
  const draggable = !selecting && (entry.kind === "file" || entry.kind === "image") && Boolean(entry.path);
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
    entry.kind === "text" && textOpen ? "is-open" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const host = entry.kind === "link" ? memoLinkHost(entry.content) : "";
  const url = entry.kind === "link" ? (entry.content ?? "") : "";
  const titleIsUrl = Boolean(url) && (entry.title === url || url.startsWith(entry.title));

  return (
    <article
      className={className}
      draggable={draggable}
      onDragStart={(event) => {
        if (!entry.path) return;
        event.dataTransfer.setData(MEMO_DRAG_TYPE, entry.id);
        event.dataTransfer.setData("text/plain", entry.path);
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
      {selecting && (
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
      )}
      {entry.kind === "image" && entry.path && !imageBroken ? (
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
            <CardFoot entry={entry} selecting={selecting} onDelete={onDelete} />
          </div>
        </>
      ) : entry.kind === "text" ? (
        <>
          <p className="memo-box__note">{entry.content || entry.title}</p>
          <CardFoot entry={entry} selecting={selecting} onDelete={onDelete} />
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
          <CardFoot entry={entry} selecting={selecting} onDelete={onDelete} />
        </>
      ) : (
        <>
          <div className="memo-box__file">
            <span className="memo-box__icon" aria-hidden>
              {kindIcon(entry.kind)}
            </span>
            <span className="memo-box__body">
              <span className="memo-box__title">{entry.title}</span>
              {fileMeta(entry) && <span className="memo-box__meta">{fileMeta(entry)}</span>}
            </span>
          </div>
          <CardFoot entry={entry} selecting={selecting} onDelete={onDelete} />
        </>
      )}
    </article>
  );
}

function CardFoot({
  entry,
  selecting,
  onDelete,
}: {
  entry: MemoEntry;
  selecting: boolean;
  onDelete: () => void;
}) {
  const t = useT();
  return (
    <div className="memo-box__foot">
      <span className="memo-box__age">{formatMemoAge(entry.createdAt)}</span>
      {!selecting && (
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
          <Trash2 size={12} />
        </button>
      )}
    </div>
  );
}
