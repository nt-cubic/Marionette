import { useEffect, useState } from "react";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";
import type { ImageAttachment, ImageMark } from "../lib/imageAttachments";
import { ImageLightbox } from "./ImageLightbox";

type UserImageCardProps = {
  attachments: ImageAttachment[];
  /** Kept for API compatibility; unload-when-far was disabled (layout flicker). */
  scrollRootRef?: React.RefObject<Element | null> | null;
};

/** Renders sent image attachments with mark overlays on a You card. */
export function UserImageCard({ attachments }: UserImageCardProps) {
  if (!attachments.length) return null;
  return (
    <div className="user-images">
      {attachments.map((att) => (
        <UserImageThumb key={att.id} attachment={att} />
      ))}
    </div>
  );
}

function AttachmentMarks({ marks }: { marks: ImageMark[] }) {
  if (marks.length === 0) return null;
  return (
    <span className="user-image-card__overlay" aria-hidden>
      {marks.map((m, i) =>
        m.kind === "point" ? (
          <span
            key={m.id}
            className="user-image-card__point"
            style={{ left: `${m.nx * 100}%`, top: `${m.ny * 100}%` }}
            title={m.comment}
          >
            {i + 1}
          </span>
        ) : (
          <span
            key={m.id}
            className="user-image-card__rect"
            style={{
              left: `${m.nx * 100}%`,
              top: `${m.ny * 100}%`,
              width: `${m.nw * 100}%`,
              height: `${m.nh * 100}%`,
            }}
            title={m.comment}
          >
            <span className="user-image-card__rect-label">{i + 1}</span>
          </span>
        ),
      )}
    </span>
  );
}

function UserImageThumb({ attachment }: { attachment: ImageAttachment }) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    if (!isTauriRuntime()) {
      setError("preview unavailable");
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

  return (
    <div className="user-image-card">
      <button
        type="button"
        className="user-image-card__frame"
        onClick={() => {
          if (dataUrl) setOpen(true);
        }}
        title={attachment.path}
      >
        {error && <span className="user-image-card__error">{attachment.name}</span>}
        {!error && !dataUrl && <span className="user-image-card__muted">…</span>}
        {dataUrl && (
          <>
            <img
              src={dataUrl}
              alt={attachment.name}
              className="user-image-card__img"
            />
            <AttachmentMarks marks={attachment.marks} />
          </>
        )}
      </button>
      {open && dataUrl ? (
        <ImageLightbox
          src={dataUrl}
          alt={attachment.name}
          caption={attachment.path}
          onClose={() => setOpen(false)}
        >
          <AttachmentMarks marks={attachment.marks} />
        </ImageLightbox>
      ) : null}
    </div>
  );
}
