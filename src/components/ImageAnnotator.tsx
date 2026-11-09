import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import {
  annotationProgress,
  clamp01,
  marksByIdOf,
  markFromPending,
  type ImageAttachment,
  type ImageMark,
  type PendingMark,
} from "../lib/imageAttachments";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";
import { useT } from "../lib/i18n";
import { DialogShell } from "./DialogShell";

type Props = {
  /** Every image in the composer: the annotator pages through all of them. */
  attachments: ImageAttachment[];
  /** Attachment the dialog opens on. */
  startId: string;
  onClose: () => void;
  /** Called once, on 完成, with the marks of every image (keyed by attachment id). */
  onSave: (marksById: Record<string, ImageMark[]>) => void;
  /** Test seam: replaces the Tauri file read (the browser preview has no file access). */
  loadImage?: (path: string) => Promise<string>;
};

type DragState = {
  startX: number;
  startY: number;
  curX: number;
  curY: number;
};

type DataUrlCache = Map<string, string>;
/** Decoded images kept while the dialog is open (stage + filmstrip share them). */
const CACHE_CAP = 40;

function loadDataUrl(cache: DataUrlCache, path: string, load: (path: string) => Promise<string>): Promise<string> {
  const hit = cache.get(path);
  if (hit) return Promise.resolve(hit);
  return load(path).then((url) => {
    cache.set(path, url);
    if (cache.size > CACHE_CAP) {
      const oldest = cache.keys().next().value;
      if (oldest !== undefined) cache.delete(oldest);
    }
    return url;
  });
}

const readFromDisk = (path: string): Promise<string> => readImageDataUrl(path).then((res) => res.dataUrl);

/**
 * Full-screen image annotator: click = point, drag = rect, then write a comment.
 * Edits every attached image in one session: ← → (or the filmstrip) switch image,
 * marks are kept per image, and 完成 saves them all at once.
 */
export function ImageAnnotator({ attachments, startId, onClose, onSave, loadImage }: Props) {
  const t = useT();
  const load = loadImage ?? readFromDisk;
  const total = attachments.length;
  const [index, setIndex] = useState(() => Math.max(0, attachments.findIndex((a) => a.id === startId)));
  const attachment = attachments[Math.min(index, total - 1)];
  const [marksById, setMarksById] = useState<Record<string, ImageMark[]>>(() => marksByIdOf(attachments));
  const marks = (attachment && marksById[attachment.id]) || [];
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingMark | null>(null);
  const [comment, setComment] = useState("");
  const [drag, setDrag] = useState<DragState | null>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const commentRef = useRef<HTMLInputElement>(null);
  const cacheRef = useRef<DataUrlCache>(new Map());

  const path = attachment?.path ?? "";
  useEffect(() => {
    let cancelled = false;
    setError(null);
    const cached = cacheRef.current.get(path);
    setDataUrl(cached ?? null);
    if (cached || !path) return;
    if (!loadImage && !isTauriRuntime()) {
      setError(t("v2dialogs.desktopOnly"));
      return;
    }
    void loadDataUrl(cacheRef.current, path, load)
      .then((url) => {
        if (!cancelled) setDataUrl(url);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
    // `t` / `load` identities do not matter for which file is shown.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  useEffect(() => {
    if (pending) {
      requestAnimationFrame(() => commentRef.current?.focus());
    }
  }, [pending]);

  const clientToNorm = useCallback((clientX: number, clientY: number) => {
    const img = imgRef.current;
    if (!img) return { nx: 0, ny: 0 };
    const rect = img.getBoundingClientRect();
    const nx = clamp01((clientX - rect.left) / Math.max(1, rect.width));
    const ny = clamp01((clientY - rect.top) / Math.max(1, rect.height));
    return { nx, ny };
  }, []);

  const onPointerDown = (e: ReactPointerEvent<HTMLImageElement>) => {
    if (pending) return;
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const { nx, ny } = clientToNorm(e.clientX, e.clientY);
    setDrag({ startX: nx, startY: ny, curX: nx, curY: ny });
  };

  const onPointerMove = (e: ReactPointerEvent<HTMLImageElement>) => {
    if (!drag) return;
    const { nx, ny } = clientToNorm(e.clientX, e.clientY);
    setDrag({ ...drag, curX: nx, curY: ny });
  };

  const onPointerUp = (e: ReactPointerEvent<HTMLImageElement>) => {
    if (!drag) return;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
    const { nx, ny } = clientToNorm(e.clientX, e.clientY);
    const dx = Math.abs(nx - drag.startX);
    const dy = Math.abs(ny - drag.startY);
    // Small movement → point; otherwise rect.
    if (dx < 0.012 && dy < 0.012) {
      setPending({ kind: "point", nx: drag.startX, ny: drag.startY });
    } else {
      const left = Math.min(drag.startX, nx);
      const top = Math.min(drag.startY, ny);
      const nw = Math.abs(nx - drag.startX);
      const nh = Math.abs(ny - drag.startY);
      setPending({ kind: "rect", nx: left, ny: top, nw, nh });
    }
    setDrag(null);
    setComment("");
  };

  /** marksById with the in-progress mark folded in (only when it already has a comment). */
  const withPending = (): Record<string, ImageMark[]> => {
    if (!attachment || !pending) return marksById;
    const mark = markFromPending(pending, comment);
    if (!mark) return marksById;
    return { ...marksById, [attachment.id]: [...(marksById[attachment.id] ?? []), mark] };
  };

  const commitPending = () => {
    if (!pending || !comment.trim()) return;
    setMarksById(withPending());
    setPending(null);
    setComment("");
  };

  const removeMark = (id: string) => {
    if (!attachment) return;
    setMarksById((cur) => ({ ...cur, [attachment.id]: (cur[attachment.id] ?? []).filter((x) => x.id !== id) }));
  };

  /** Switch image. A commented-but-unconfirmed mark is kept; a bare selection is dropped. */
  const goTo = (next: number) => {
    const target = Math.min(total - 1, Math.max(0, next));
    if (target === index) return;
    setMarksById(withPending());
    setPending(null);
    setComment("");
    setDrag(null);
    setIndex(target);
  };
  const goToRef = useRef(goTo);
  goToRef.current = goTo;

  // ← / → switch image, unless the user is typing a comment.
  useEffect(() => {
    if (total < 2) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing || e.altKey || e.ctrlKey || e.metaKey) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)) return;
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        goToRef.current(index - 1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        goToRef.current(index + 1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [index, total]);

  const finish = () => {
    onSave(withPending());
    onClose();
  };

  const dragRect =
    drag &&
    (() => {
      const left = Math.min(drag.startX, drag.curX);
      const top = Math.min(drag.startY, drag.curY);
      const w = Math.abs(drag.curX - drag.startX);
      const h = Math.abs(drag.curY - drag.startY);
      return { left, top, w, h };
    })();

  if (!attachment) return null;
  const progress = annotationProgress(attachments, withPending());

  return (
    <DialogShell
      className="image-annotator-backdrop"
      dialogClassName="image-annotator"
      labelledBy="image-annotator-title"
      onClose={onClose}
    >
        <div className="image-annotator__header">
          <div>
            <strong id="image-annotator-title">{attachment.name}</strong>
            <span>{t("annotator.hint")}</span>
          </div>
          {total > 1 && (
            <span className="image-annotator__counter" aria-live="polite">
              {t("annotator.counter", { index: index + 1, total })}
            </span>
          )}
          <button
            type="button"
            className="image-annotator__icon-btn"
            title={t("common.close")}
            aria-label={t("common.close")}
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </div>

        <div className="image-annotator__body">
          <div className="image-annotator__main">
            <div className="image-annotator__stagewrap">
              {total > 1 && (
                <button
                  type="button"
                  className="image-annotator__nav is-prev"
                  title={t("annotator.prev")}
                  aria-label={t("annotator.prev")}
                  disabled={index === 0}
                  onClick={() => goTo(index - 1)}
                >
                  <ChevronLeft size={18} strokeWidth={1.5} />
                </button>
              )}
              <div className="image-annotator__stage">
                {error && <p className="image-annotator__error">{error}</p>}
                {!error && !dataUrl && <p className="image-annotator__muted">{t("v2dialogs.loadingImage")}</p>}
                {dataUrl && (
                  <div className="image-annotator__frame">
                    <img
                      ref={imgRef}
                      key={attachment.id}
                      src={dataUrl}
                      alt={attachment.name}
                      className="image-annotator__img"
                      draggable={false}
                      onPointerDown={onPointerDown}
                      onPointerMove={onPointerMove}
                      onPointerUp={onPointerUp}
                      onPointerCancel={() => setDrag(null)}
                    />
                    <div className="image-annotator__overlay" aria-hidden>
                      {marks.map((m, i) =>
                        m.kind === "point" ? (
                          <span
                            key={m.id}
                            className="image-annotator__point"
                            style={{ left: `${m.nx * 100}%`, top: `${m.ny * 100}%` }}
                            title={m.comment}
                          >
                            {i + 1}
                          </span>
                        ) : (
                          <span
                            key={m.id}
                            className="image-annotator__rect"
                            style={{
                              left: `${m.nx * 100}%`,
                              top: `${m.ny * 100}%`,
                              width: `${m.nw * 100}%`,
                              height: `${m.nh * 100}%`,
                            }}
                            title={m.comment}
                          >
                            <span className="image-annotator__rect-label">{i + 1}</span>
                          </span>
                        ),
                      )}
                      {pending?.kind === "point" && (
                        <span
                          className="image-annotator__point is-pending"
                          style={{ left: `${pending.nx * 100}%`, top: `${pending.ny * 100}%` }}
                        >
                          +
                        </span>
                      )}
                      {pending?.kind === "rect" && (
                        <span
                          className="image-annotator__rect is-pending"
                          style={{
                            left: `${pending.nx * 100}%`,
                            top: `${pending.ny * 100}%`,
                            width: `${pending.nw * 100}%`,
                            height: `${pending.nh * 100}%`,
                          }}
                        />
                      )}
                      {dragRect && (dragRect.w > 0.005 || dragRect.h > 0.005) && (
                        <span
                          className="image-annotator__rect is-dragging"
                          style={{
                            left: `${dragRect.left * 100}%`,
                            top: `${dragRect.top * 100}%`,
                            width: `${dragRect.w * 100}%`,
                            height: `${dragRect.h * 100}%`,
                          }}
                        />
                      )}
                    </div>
                  </div>
                )}
              </div>
              {total > 1 && (
                <button
                  type="button"
                  className="image-annotator__nav is-next"
                  title={t("annotator.next")}
                  aria-label={t("annotator.next")}
                  disabled={index === total - 1}
                  onClick={() => goTo(index + 1)}
                >
                  <ChevronRight size={18} strokeWidth={1.5} />
                </button>
              )}
            </div>

            {total > 1 && (
              <div className="image-annotator__strip" role="tablist" aria-label={t("annotator.stripAria")}>
                {attachments.map((att, i) => (
                  <StripThumb
                    key={att.id}
                    att={att}
                    active={i === index}
                    markCount={(marksById[att.id] ?? att.marks).length}
                    label={t("annotator.thumbAria", { index: i + 1, name: att.name })}
                    cache={cacheRef.current}
                    load={load}
                    enabled={!!loadImage || isTauriRuntime()}
                    onPick={() => goTo(i)}
                  />
                ))}
              </div>
            )}
          </div>

          <aside className="image-annotator__side">
            {pending ? (
              <div className="image-annotator__compose">
                <div className="image-annotator__compose-meta">
                  {pending.kind === "point"
                    ? t("annotator.pendingPoint", { x: pending.nx.toFixed(2), y: pending.ny.toFixed(2) })
                    : t("annotator.pendingRect", { w: pending.nw.toFixed(2), h: pending.nh.toFixed(2)})}
                </div>
                <input
                  ref={commentRef}
                  className="image-annotator__comment-input"
                  value={comment}
                  placeholder={t("annotator.commentPlaceholder")}
                  onChange={(e) => setComment(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") {
                      e.stopPropagation();
                      setPending(null);
                      setComment("");
                    }
                    if (e.key === "Enter") {
                      e.preventDefault();
                      commitPending();
                    }
                  }}
                />
                <div className="image-annotator__compose-actions">
                  <button
                    type="button"
                    className="project-dialog__cancel"
                    onClick={() => {
                      setPending(null);
                      setComment("");
                    }}
                  >
                    {t("common.cancel")}
                  </button>
                  <button
                    type="button"
                    className="project-dialog__submit"
                    disabled={!comment.trim()}
                    onClick={commitPending}
                  >
                    {t("annotator.add")}
                  </button>
                </div>
              </div>
            ) : (
              <p className="image-annotator__hint">
                {t("annotator.emptyHint")}
              </p>
            )}

            <div className="image-annotator__list-title">{t("annotator.listTitle", { count: marks.length })}</div>
            {marks.length === 0 ? (
              <p className="image-annotator__muted">{t("annotator.noMarks")}</p>
            ) : (
              <ul className="image-annotator__list">
                {marks.map((m, i) => (
                  <li key={m.id}>
                    <span className="image-annotator__list-n">{i + 1}</span>
                    <span className="image-annotator__list-text">
                      <strong>{m.kind === "point" ? t("annotator.point") : t("annotator.rect")}</strong> {m.comment}
                    </span>
                    <button
                      type="button"
                      className="image-annotator__list-del"
                      title={t("common.delete")}
                      onClick={() => removeMark(m.id)}
                    >
                      <X size={14} strokeWidth={1.5} />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </aside>
        </div>

        <div className="image-annotator__footer">
          {total > 1 && (
            <span className="image-annotator__summary">
              {t("annotator.summary", { done: progress.done, total: progress.total, marks: progress.marks })}
              <em>{t("annotator.navHint")}</em>
            </span>
          )}
          <button type="button" className="project-dialog__cancel" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="project-dialog__submit" onClick={finish}>
            {t("annotator.done")}
          </button>
        </div>
    </DialogShell>
  );
}

/** One filmstrip tile: the image, its mark count, a highlight when it is the one on stage. */
function StripThumb({
  att,
  active,
  markCount,
  label,
  cache,
  load,
  enabled,
  onPick,
}: {
  att: ImageAttachment;
  active: boolean;
  markCount: number;
  label: string;
  cache: DataUrlCache;
  load: (path: string) => Promise<string>;
  enabled: boolean;
  onPick: () => void;
}) {
  const [url, setUrl] = useState<string | null>(() => cache.get(att.path) ?? null);
  useEffect(() => {
    let cancelled = false;
    if (url || !enabled) return;
    void loadDataUrl(cache, att.path, load)
      .then((u) => {
        if (!cancelled) setUrl(u);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
    // The tile loads once per path.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [att.path, enabled]);
  const initial = (att.name.replace(/\.[^.]+$/, "").trim()[0] || "?").toUpperCase();
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      aria-label={label}
      title={att.name}
      className={`image-annotator__thumb${active ? " is-active" : ""}`}
      onClick={onPick}
    >
      {url ? <img src={url} alt="" draggable={false} /> : <span aria-hidden>{initial}</span>}
      {markCount > 0 && <b className="image-annotator__thumb-badge">{markCount}</b>}
    </button>
  );
}
