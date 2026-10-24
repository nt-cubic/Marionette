/**
 * Memo inbox overlay (edge dock stage 3, docs/plan-edge-dock.md §4 / §4.1;
 * layout per docs/design/edge-dock.html ③④).
 *
 * 收 — Explorer drag-in (tauri onDragDropEvent hit-tested against this overlay)
 *      and Ctrl+V of text / a bare URL.
 * 找 — search + kind filters.
 * 用 — drag a file row onto the composer, or pick several (选择 / Ctrl-click)
 *      and hand them to a new dialog: as context, or to be organized into a
 *      note under docs/notes/. Both only prefill the composer.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Check, File as FileIcon, FileText, Image as ImageIcon, Link2, MessageSquarePlus, Search, Trash2, Type, X } from "lucide-react";
import { isTauriRuntime } from "../lib/api";
import {
  formatMemoAge,
  formatMemoSize,
  MEMO_DRAG_TYPE,
  memoDelete,
  memoList,
  memoSavePaths,
  memoSaveText,
  type MemoEntry,
  type MemoKind,
} from "../lib/memos";
import { showToast } from "../lib/toast";

const FILTERS: ReadonlyArray<{ id: MemoKind | "all"; label: string }> = [
  { id: "all", label: "全部" },
  { id: "link", label: "链接" },
  { id: "text", label: "文字" },
  { id: "image", label: "图片" },
  { id: "file", label: "文件" },
];

function kindIcon(kind: MemoKind) {
  if (kind === "link") return <Link2 size={12} />;
  if (kind === "image") return <ImageIcon size={12} />;
  if (kind === "text") return <Type size={12} />;
  return <FileIcon size={12} />;
}

/** One quiet meta line: host for links, size for files, first words for text. */
function metaLine(entry: MemoEntry): string {
  if (entry.kind === "link") {
    try {
      return new URL(entry.content ?? "").host;
    } catch {
      return entry.content ?? "";
    }
  }
  if (entry.kind === "text") {
    // The title is the text's first line; show what follows it.
    const flat = (entry.content ?? "").replace(/\s+/g, " ").trim();
    const rest = flat.startsWith(entry.title) ? flat.slice(entry.title.length).replace(/^[\s/·,，。:：-]+/, "") : flat;
    return rest.slice(0, 80);
  }
  return [formatMemoSize(entry.size), entry.source].filter(Boolean).join(" · ");
}

type Props = {
  onClose: () => void;
  onChanged: (count: number) => void;
  /** Hand the picked entries to a new dialog (prefilled, not sent). */
  onUse: (entries: MemoEntry[], intent: "dialog" | "doc") => void;
  /** Floating window: no dock title bar, so the page carries its own close. */
  showClose?: boolean;
};

export function MemoBox({ onClose, onChanged, onUse, showClose = false }: Props) {
  const [entries, setEntries] = useState<MemoEntry[]>([]);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<MemoKind | "all">("all");
  const [dropActive, setDropActive] = useState(false);
  const [busy, setBusy] = useState(false);
  const [selecting, setSelecting] = useState(false);
  const [picked, setPicked] = useState<ReadonlySet<string>>(() => new Set());
  const rootRef = useRef<HTMLDivElement>(null);
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

  // Esc leaves select mode first, then closes; focus stays here for Ctrl+V.
  useEffect(() => {
    rootRef.current?.focus();
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

  // Explorer drag-in: native drops give absolute paths; hit-test against this
  // overlay so a drop elsewhere keeps its existing meaning.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (disposed) return;
        const payload = event.payload;
        const el = rootRef.current;
        if (!el) return;
        if (payload.type === "leave") {
          setDropActive(false);
          return;
        }
        const pos = "position" in payload ? payload.position : null;
        if (!pos) return;
        const rect = el.getBoundingClientRect();
        const scale = window.devicePixelRatio || 1;
        const x = pos.x / scale;
        const y = pos.y / scale;
        const over = x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
        if (payload.type === "enter" || payload.type === "over") {
          setDropActive(over);
          return;
        }
        if (payload.type === "drop") {
          setDropActive(false);
          if (!over || !payload.paths?.length) return;
          setBusy(true);
          void memoSavePaths(payload.paths, "拖入")
            .then(publish)
            .catch((error) => showToast({ text: "没能存进收纳盒", detail: String(error), tone: "error" }))
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

  const onPaste = useCallback(
    (event: React.ClipboardEvent) => {
      // The search box keeps its own paste.
      if ((event.target as HTMLElement).tagName === "INPUT") return;
      const text = event.clipboardData.getData("text/plain").trim();
      if (!text) return;
      event.preventDefault();
      setBusy(true);
      void memoSaveText(text, undefined, "粘贴")
        .then((saved) => {
          if (!saved) {
            showToast({ text: "没能存进收纳盒", tone: "error" });
            return;
          }
          return memoList().then(publish);
        })
        .finally(() => setBusy(false));
    },
    [publish],
  );

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return entries.filter((entry) => {
      if (filter !== "all" && entry.kind !== filter) return false;
      if (!q) return true;
      const haystack = [entry.title, entry.content ?? "", entry.path ?? ""].join("\n").toLowerCase();
      return haystack.includes(q);
    });
  }, [entries, filter, query]);

  const counts = useMemo(() => {
    const map = new Map<MemoKind | "all", number>([["all", entries.length]]);
    for (const entry of entries) map.set(entry.kind, (map.get(entry.kind) ?? 0) + 1);
    return map;
  }, [entries]);

  const remove = (id: string) => {
    void memoDelete(id).then((next) => {
      if (next) publish(next);
      else showToast({ text: "删除失败，条目还在", tone: "error" });
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

  return (
    <div
      ref={rootRef}
      className={`memo-box${dropActive ? " is-drop-active" : ""}${selecting ? " is-selecting" : ""}`}
      role="dialog"
      aria-label="收纳盒"
      tabIndex={-1}
      onPaste={onPaste}
    >
      {/* Title and 返回 live in the strip's title bar. */}
      <div className="memo-box__head">
        {showClose && <strong className="memo-box__title-label">收纳盒</strong>}
        <label className="memo-box__search">
          <Search size={12} />
          <input
            type="search"
            placeholder="搜索标题、内容、路径"
            aria-label="搜索收纳盒"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          {busy && <span className="memo-box__busy">存入中…</span>}
        </label>
        {entries.length > 0 && (
          <button
            type="button"
            className={selecting ? "memo-box__text-btn is-on" : "memo-box__text-btn"}
            onClick={() => (selecting ? exitSelect() : setSelecting(true))}
          >
            {selecting ? "完成" : "选择"}
          </button>
        )}
        {showClose && (
          <button
            type="button"
            className="memo-box__close"
            title="关闭 · Esc"
            aria-label="关闭收纳盒"
            onClick={onClose}
          >
            <X size={13} />
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
              {chip.label}
              <b>{counts.get(chip.id) ?? 0}</b>
            </button>
          ))}
        </div>
      )}
      <div className="memo-box__list custom-scrollbar">
        {entries.length === 0 ? (
          <div className="memo-box__empty">
            <strong>还没有东西</strong>
            <span>把文件、文件夹拖到这里，或 Ctrl V 粘贴文字和链接。</span>
            <span>之后可以多选，交给新对话，或整理成一篇笔记。</span>
          </div>
        ) : filtered.length === 0 ? (
          <p className="memo-box__none">没有匹配的条目</p>
        ) : (
          filtered.map((entry) => {
            const isPicked = picked.has(entry.id);
            const draggable = !selecting && (entry.kind === "file" || entry.kind === "image") && Boolean(entry.path);
            return (
              <div
                key={entry.id}
                className={isPicked ? "memo-box__row is-picked" : "memo-box__row"}
                draggable={draggable}
                onDragStart={(event) => {
                  if (!entry.path) return;
                  event.dataTransfer.setData(MEMO_DRAG_TYPE, entry.id);
                  event.dataTransfer.setData("text/plain", entry.path);
                  event.dataTransfer.effectAllowed = "copy";
                }}
                onClick={(event) => {
                  if (selecting) toggle(entry.id);
                  else if (event.ctrlKey || event.metaKey) {
                    setSelecting(true);
                    toggle(entry.id);
                  }
                }}
                title={
                  entry.kind === "link" || entry.kind === "text"
                    ? (entry.content ?? entry.title)
                    : `${entry.path ?? entry.title}${draggable ? "\n可拖到输入框" : ""}`
                }
              >
                <span className="memo-box__icon" aria-hidden>
                  {selecting ? (
                    <span className={isPicked ? "memo-box__check is-on" : "memo-box__check"}>
                      {isPicked && <Check size={10} strokeWidth={3} />}
                    </span>
                  ) : (
                    kindIcon(entry.kind)
                  )}
                </span>
                <span className="memo-box__body">
                  <span className="memo-box__title">{entry.title}</span>
                  {metaLine(entry) && <span className="memo-box__meta">{metaLine(entry)}</span>}
                </span>
                <span className="memo-box__age">{formatMemoAge(entry.createdAt)}</span>
                {!selecting && (
                  <button
                    type="button"
                    className="memo-box__delete"
                    aria-label={`删除 ${entry.title}`}
                    title="删除"
                    onClick={(event) => {
                      event.stopPropagation();
                      remove(entry.id);
                    }}
                  >
                    <Trash2 size={12} />
                  </button>
                )}
              </div>
            );
          })
        )}
      </div>
      {selecting ? (
        <div className="memo-box__selbar">
          <span>
            已选 <b>{picked.size}</b>
          </span>
          <span className="memo-box__spacer" />
          <button type="button" className="memo-box__act" disabled={!picked.size} onClick={() => use("doc")}>
            <FileText size={12} />
            整理成文档
          </button>
          <button type="button" className="memo-box__act is-primary" disabled={!picked.size} onClick={() => use("dialog")}>
            <MessageSquarePlus size={12} />
            开新对话
          </button>
        </div>
      ) : (
        <div className={dropActive ? "memo-box__drop is-active" : "memo-box__drop"}>
          {dropActive ? "松开即存入" : "拖入文件 · Ctrl V 粘贴 · Ctrl 点击多选"}
        </div>
      )}
    </div>
  );
}
