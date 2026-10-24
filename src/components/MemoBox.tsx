/**
 * Memo inbox overlay (edge dock stage 3, docs/plan-edge-dock.md §4).
 *
 * Three intake channels (v1):
 *  - Explorer drag-in: tauri onDragDropEvent hit-tested against this overlay
 *    (same pattern as Composer's field), paths land in memo_save_paths;
 *  - Ctrl+V anywhere in the overlay: text / bare URL → memo_save_text;
 *  - drag OUT to the composer: file/image rows are HTML5-draggable and carry
 *    their absolute path — the composer accepts in-page memo drags.
 *
 * The §4.1 karakeep-lite layer (multi-select, AI tags, organize-into-doc,
 * new-dialog-from-picked) is the follow-up on top of this deterministic box.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { File as FileIcon, Image as ImageIcon, Link2, Search, Trash2, Type, X } from "lucide-react";
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

function kindTag(kind: MemoKind): string {
  if (kind === "link") return "URL";
  if (kind === "image") return "IMG";
  if (kind === "text") return "TXT";
  return "FILE";
}

export function MemoBox({ onClose, onChanged }: { onClose: () => void; onChanged: (count: number) => void }) {
  const [entries, setEntries] = useState<MemoEntry[]>([]);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<MemoKind | "all">("all");
  const [dropActive, setDropActive] = useState(false);
  const [busy, setBusy] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  const publish = useCallback(
    (next: MemoEntry[]) => {
      setEntries(next);
      onChanged(next.length);
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

  // Esc closes; the overlay keeps keyboard focus so Ctrl+V lands here.
  useEffect(() => {
    rootRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
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
            .catch(() => undefined)
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
      const text = event.clipboardData.getData("text/plain").trim();
      if (!text) return;
      event.preventDefault();
      setBusy(true);
      void memoSaveText(text, undefined, "粘贴")
        .then(() => memoList())
        .then(publish)
        .catch(() => undefined)
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
    for (const entry of entries) {
      map.set(entry.kind, (map.get(entry.kind) ?? 0) + 1);
    }
    return map;
  }, [entries]);

  const remove = (id: string) => {
    void memoDelete(id).then(publish);
  };

  return (
    <div
      ref={rootRef}
      className={dropActive ? "memo-box is-drop-active" : "memo-box"}
      role="dialog"
      aria-label="收纳盒"
      tabIndex={-1}
      onPaste={onPaste}
    >
      <div className="memo-box__head">
        <strong>收纳盒</strong>
        <span className="memo-box__count">{entries.length} 条</span>
        <span className="memo-box__spacer" />
        <button type="button" className="memo-box__close" aria-label="关闭收纳盒" onClick={onClose}>
          <X size={13} />
        </button>
      </div>
      <div className="memo-box__toolbar">
        <Search size={12} />
        <input
          type="search"
          placeholder="搜索标题、内容、路径…"
          aria-label="搜索收纳盒"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </div>
      <div className="memo-box__filters">
        {FILTERS.map((chip) => (
          <button
            key={chip.id}
            type="button"
            className={filter === chip.id ? "memo-box__chip is-on" : "memo-box__chip"}
            onClick={() => setFilter(chip.id)}
          >
            {chip.label} <b>{counts.get(chip.id) ?? 0}</b>
          </button>
        ))}
        {busy && <span className="memo-box__busy">处理中…</span>}
      </div>
      <div className="memo-box__list custom-scrollbar">
        {filtered.length === 0 ? (
          <p className="memo-box__empty">{entries.length === 0 ? "空收纳盒 · 拖文件到这里或 Ctrl+V 粘贴文字/链接" : "没有匹配的条目"}</p>
        ) : (
          filtered.map((entry) => (
            <div
              key={entry.id}
              className="memo-box__row"
              draggable={entry.kind === "file" || entry.kind === "image"}
              onDragStart={(event) => {
                if (!entry.path) return;
                event.dataTransfer.setData(MEMO_DRAG_TYPE, entry.id);
                event.dataTransfer.setData("text/plain", entry.path);
                event.dataTransfer.effectAllowed = "copy";
              }}
              title={
                entry.kind === "link" || entry.kind === "text"
                  ? entry.content ?? entry.title
                  : entry.path ?? entry.title
              }
            >
              <span className="memo-box__icon">{kindIcon(entry.kind)}</span>
              <span className="memo-box__body">
                <span className="memo-box__title">{entry.title}</span>
                <span className="memo-box__meta">
                  <i>{kindTag(entry.kind)}</i>
                  {formatMemoSize(entry.size)}
                  {entry.source ? ` · ${entry.source}` : ""}
                  {(entry.kind === "file" || entry.kind === "image") && " · 可拖到输入框"}
                </span>
              </span>
              <span className="memo-box__age">{formatMemoAge(entry.createdAt)}</span>
              <button
                type="button"
                className="memo-box__delete"
                aria-label={`删除 ${entry.title}`}
                title="删除"
                onClick={() => remove(entry.id)}
              >
                <Trash2 size={12} />
              </button>
            </div>
          ))
        )}
      </div>
      <div className={dropActive ? "memo-box__drop is-active" : "memo-box__drop"}>
        拖文件/文件夹到这里存进收纳盒 · Ctrl V 粘贴文字或链接
      </div>
    </div>
  );
}
