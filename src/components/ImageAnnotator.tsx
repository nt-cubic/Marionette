import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { X } from "lucide-react";
import {
  clamp01,
  newMarkId,
  type ImageAttachment,
  type ImageMark,
} from "../lib/imageAttachments";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";

type Props = {
  attachment: ImageAttachment;
  onClose: () => void;
  onSave: (marks: ImageMark[]) => void;
};

type DragState = {
  startX: number;
  startY: number;
  curX: number;
  curY: number;
};

/**
 * Full-screen image annotator: click = point, drag = rect, then write a comment.
 */
export function ImageAnnotator({ attachment, onClose, onSave }: Props) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [marks, setMarks] = useState<ImageMark[]>(attachment.marks);
  const [pending, setPending] = useState<
    | { kind: "point"; nx: number; ny: number }
    | { kind: "rect"; nx: number; ny: number; nw: number; nh: number }
    | null
  >(null);
  const [comment, setComment] = useState("");
  const [drag, setDrag] = useState<DragState | null>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const commentRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setDataUrl(null);
    if (!isTauriRuntime()) {
      setError("Image preview requires the desktop app");
      return;
    }
    void readImageDataUrl(attachment.path)
      .then((res) => {
        if (!cancelled) setDataUrl(res.dataUrl);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [attachment.path]);

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

  const commitPending = () => {
    if (!pending || !comment.trim()) return;
    const id = newMarkId();
    const next: ImageMark =
      pending.kind === "point"
        ? {
            id,
            kind: "point",
            nx: pending.nx,
            ny: pending.ny,
            comment: comment.trim(),
          }
        : {
            id,
            kind: "rect",
            nx: pending.nx,
            ny: pending.ny,
            nw: pending.nw,
            nh: pending.nh,
            comment: comment.trim(),
          };
    setMarks((m) => [...m, next]);
    setPending(null);
    setComment("");
  };

  const removeMark = (id: string) => {
    setMarks((m) => m.filter((x) => x.id !== id));
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

  return (
    <div className="image-annotator-backdrop" role="presentation">
      <div
        className="image-annotator"
        role="dialog"
        aria-modal="true"
        aria-labelledby="image-annotator-title"
      >
        <div className="image-annotator__header">
          <div>
            <strong id="image-annotator-title">{attachment.name}</strong>
            <span>单击打点 · 拖拽框选 · 写评论</span>
          </div>
          <button
            type="button"
            className="image-annotator__icon-btn"
            title="Close"
            aria-label="Close"
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </div>

        <div className="image-annotator__body">
          <div className="image-annotator__stage">
            {error && <p className="image-annotator__error">{error}</p>}
            {!error && !dataUrl && <p className="image-annotator__muted">Loading image…</p>}
            {dataUrl && (
              <div className="image-annotator__frame">
                <img
                  ref={imgRef}
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

          <aside className="image-annotator__side">
            {pending ? (
              <div className="image-annotator__compose">
                <div className="image-annotator__compose-meta">
                  {pending.kind === "point"
                    ? `点 (${pending.nx.toFixed(2)}, ${pending.ny.toFixed(2)})`
                    : `框 ${pending.nw.toFixed(2)}×${pending.nh.toFixed(2)}`}
                </div>
                <input
                  ref={commentRef}
                  className="image-annotator__comment-input"
                  value={comment}
                  placeholder="写评论，回车确认…"
                  onChange={(e) => setComment(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") {
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
                    取消
                  </button>
                  <button
                    type="button"
                    className="project-dialog__submit"
                    disabled={!comment.trim()}
                    onClick={commitPending}
                  >
                    添加
                  </button>
                </div>
              </div>
            ) : (
              <p className="image-annotator__hint">
                在图上单击打点，或拖拽框选区域，然后写评论。
              </p>
            )}

            <div className="image-annotator__list-title">批注 ({marks.length})</div>
            {marks.length === 0 ? (
              <p className="image-annotator__muted">还没有批注</p>
            ) : (
              <ul className="image-annotator__list">
                {marks.map((m, i) => (
                  <li key={m.id}>
                    <span className="image-annotator__list-n">{i + 1}</span>
                    <span className="image-annotator__list-text">
                      <strong>{m.kind === "point" ? "点" : "框"}</strong> {m.comment}
                    </span>
                    <button
                      type="button"
                      className="image-annotator__list-del"
                      title="删除"
                      onClick={() => removeMark(m.id)}
                    >
                      ✕
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </aside>
        </div>

        <div className="image-annotator__footer">
          <button type="button" className="project-dialog__cancel" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="project-dialog__submit"
            onClick={() => {
              onSave(marks);
              onClose();
            }}
          >
            完成
          </button>
        </div>
      </div>
    </div>
  );
}
