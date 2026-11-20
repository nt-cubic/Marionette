import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { X } from "lucide-react";
import {
  clamp01,
  clampImageCropRect,
  FULL_IMAGE_CROP,
  isFullImageCrop,
  newMarkId,
  remapMarksForCrop,
  resizeImageCrop,
  type ImageAttachment,
  type ImageCropHandle,
  type ImageCropRect,
  type ImageMark,
} from "../lib/imageAttachments";
import { isTauriRuntime, readImageDataUrl, savePastedImage } from "../lib/api";
import { useT } from "../lib/i18n";
import { DialogShell } from "./DialogShell";

type Props = {
  attachment: ImageAttachment;
  onClose: () => void;
  onSave: (marks: ImageMark[], cropped?: ImageAttachment) => void;
  /** Replaces the footer action label when the result goes somewhere specific. */
  saveLabel?: string;
};

type DragState = {
  startX: number;
  startY: number;
  curX: number;
  curY: number;
};

type CropDragState = {
  handle: ImageCropHandle;
  pointerId: number;
  startPointerX: number;
  startPointerY: number;
  startRect: ImageCropRect;
};

const CROP_HANDLES: ImageCropHandle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

async function cropImageDataUrl(
  dataUrl: string,
  crop: ImageCropRect,
): Promise<{ dataUrl: string; width: number; height: number }> {
  const image = new Image();
  image.src = dataUrl;
  await image.decode();
  const box = clampImageCropRect(crop);
  const sx = Math.max(0, Math.min(image.naturalWidth - 1, Math.round(box.nx * image.naturalWidth)));
  const sy = Math.max(0, Math.min(image.naturalHeight - 1, Math.round(box.ny * image.naturalHeight)));
  const width = Math.max(1, Math.min(image.naturalWidth - sx, Math.round(box.nw * image.naturalWidth)));
  const height = Math.max(1, Math.min(image.naturalHeight - sy, Math.round(box.nh * image.naturalHeight)));
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Canvas is unavailable");
  ctx.drawImage(image, sx, sy, width, height, 0, 0, width, height);
  return { dataUrl: canvas.toDataURL("image/png"), width, height };
}

/**
 * Full-screen image annotator: click = point, drag = rect, then write a comment.
 */
export function ImageAnnotator({ attachment, onClose, onSave, saveLabel }: Props) {
  const t = useT();
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [marks, setMarks] = useState<ImageMark[]>(() => attachment.marks ?? []);
  const [pending, setPending] = useState<
    | { kind: "point"; nx: number; ny: number }
    | { kind: "rect"; nx: number; ny: number; nw: number; nh: number }
    | null
  >(null);
  const [comment, setComment] = useState("");
  const [drag, setDrag] = useState<DragState | null>(null);
  const [crop, setCrop] = useState<ImageCropRect>({ ...FULL_IMAGE_CROP });
  const [imageSize, setImageSize] = useState<{ width: number; height: number } | null>(null);
  const [saving, setSaving] = useState(false);
  const [cropError, setCropError] = useState<string | null>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const commentRef = useRef<HTMLInputElement>(null);
  const cropDragRef = useRef<CropDragState | null>(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setDataUrl(null);
    setCrop({ ...FULL_IMAGE_CROP });
    setImageSize(null);
    setCropError(null);
    if (!isTauriRuntime()) {
      setError(t("v2dialogs.desktopOnly"));
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

  const onCropPointerDown = (
    e: ReactPointerEvent<HTMLButtonElement>,
    handle: ImageCropHandle,
  ) => {
    if (e.button !== 0 || saving) return;
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    const { nx, ny } = clientToNorm(e.clientX, e.clientY);
    cropDragRef.current = {
      handle,
      pointerId: e.pointerId,
      startPointerX: nx,
      startPointerY: ny,
      startRect: crop,
    };
  };

  const onCropPointerMove = (e: ReactPointerEvent<HTMLButtonElement>) => {
    const active = cropDragRef.current;
    if (!active || active.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    const { nx, ny } = clientToNorm(e.clientX, e.clientY);
    setCrop(
      resizeImageCrop(
        active.startRect,
        active.handle,
        nx - active.startPointerX,
        ny - active.startPointerY,
      ),
    );
  };

  const onCropPointerUp = (e: ReactPointerEvent<HTMLButtonElement>) => {
    const active = cropDragRef.current;
    if (!active || active.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
    cropDragRef.current = null;
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLImageElement>) => {
    if (pending || saving) return;
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

  const cropChanged = !isFullImageCrop(crop);
  const cropWidth = imageSize ? Math.max(1, Math.round(imageSize.width * crop.nw)) : 0;
  const cropHeight = imageSize ? Math.max(1, Math.round(imageSize.height * crop.nh)) : 0;

  const finishSave = async () => {
    if (saving) return;
    setSaving(true);
    setCropError(null);
    try {
      let croppedAttachment: ImageAttachment | undefined;
      let savedMarks = marks;
      if (cropChanged && dataUrl) {
        const result = await cropImageDataUrl(dataUrl, crop);
        const saved = await savePastedImage(result.dataUrl, "image/png");
        const base = attachment.name.replace(/\.[^.]+$/, "") || attachment.name;
        croppedAttachment = {
          id: attachment.id,
          path: saved.path,
          name: `${base}-cropped.png`,
          mimeType: "image/png",
          marks: [],
        };
        savedMarks = remapMarksForCrop(marks, crop);
      }
      onSave(savedMarks, croppedAttachment);
      onClose();
    } catch (err) {
      setCropError(
        t("annotator.cropFailed", {
          message: err instanceof Error ? err.message : String(err),
        }),
      );
      setSaving(false);
    }
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
          <div className="image-annotator__stage">
            {error && <p className="image-annotator__error">{error}</p>}
            {!error && !dataUrl && <p className="image-annotator__muted">{t("v2dialogs.loadingImage")}</p>}
            {dataUrl && (
              <div className="image-annotator__frame">
                <img
                  ref={imgRef}
                  src={dataUrl}
                  alt={attachment.name}
                  className="image-annotator__img"
                  draggable={false}
                  onLoad={(e) =>
                    setImageSize({
                      width: e.currentTarget.naturalWidth,
                      height: e.currentTarget.naturalHeight,
                    })
                  }
                  onPointerDown={onPointerDown}
                  onPointerMove={onPointerMove}
                  onPointerUp={onPointerUp}
                  onPointerCancel={() => setDrag(null)}
                />
                <div className="image-annotator__overlay">
                  <div
                    className={`image-annotator__crop${cropChanged ? " is-active" : ""}`}
                    style={{
                      left: `${crop.nx * 100}%`,
                      top: `${crop.ny * 100}%`,
                      width: `${crop.nw * 100}%`,
                      height: `${crop.nh * 100}%`,
                    }}
                  >
                    {CROP_HANDLES.map((handle) => (
                      <button
                        key={handle}
                        type="button"
                        className={`image-annotator__crop-handle is-${handle}`}
                        title={t("annotator.cropHandle")}
                        aria-label={t("annotator.cropHandle")}
                        disabled={saving}
                        onPointerDown={(event) => onCropPointerDown(event, handle)}
                        onPointerMove={onCropPointerMove}
                        onPointerUp={onCropPointerUp}
                        onPointerCancel={onCropPointerUp}
                      />
                    ))}
                  </div>
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
            <div className="image-annotator__crop-panel">
              <div className="image-annotator__crop-panel-head">
                <strong>{t("annotator.cropTitle")}</strong>
                <span>
                  {imageSize
                    ? t("annotator.cropSize", { w: cropWidth, h: cropHeight })
                    : ""}
                </span>
              </div>
              <p>{t("annotator.cropHint")}</p>
              <button
                type="button"
                className="project-dialog__cancel"
                disabled={!cropChanged || saving}
                onClick={() => setCrop({ ...FULL_IMAGE_CROP })}
              >
                {t("annotator.cropReset")}
              </button>
              {cropError && <p className="image-annotator__error">{cropError}</p>}
            </div>
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
          <button type="button" className="project-dialog__cancel" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="project-dialog__submit"
            disabled={saving || !!error || !dataUrl}
            onClick={() => void finishSave()}
          >
            {saving ? t("annotator.cropping") : saveLabel ?? t("annotator.done")}
          </button>
        </div>
    </DialogShell>
  );
}
