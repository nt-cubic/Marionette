import { useEffect, useState } from "react";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";
import type { ImageAttachment } from "../lib/imageAttachments";

/** Renders sent image attachments with mark overlays on a You card. */
export function UserImageCard({ attachments }: { attachments: ImageAttachment[] }) {
  if (!attachments.length) return null;
  return (
    <div className="user-images">
      {attachments.map((att) => (
        <UserImageThumb key={att.id} attachment={att} />
      ))}
    </div>
  );
}

function UserImageThumb({ attachment }: { attachment: ImageAttachment }) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);

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
        onClick={() => setExpanded((v) => !v)}
        title={attachment.path}
      >
        {error && <span className="user-image-card__error">{attachment.name}</span>}
        {!error && !dataUrl && <span className="user-image-card__muted">…</span>}
        {dataUrl && (
          <>
            <img src={dataUrl} alt={attachment.name} className="user-image-card__img" />
            <span className="user-image-card__overlay" aria-hidden>
              {attachment.marks.map((m, i) =>
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
          </>
        )}
      </button>
      <div className="user-image-card__meta">
        <span className="user-image-card__name">{attachment.name}</span>
        {attachment.marks.length > 0 && (
          <span className="user-image-card__count">{attachment.marks.length} 标注</span>
        )}
      </div>
      {expanded && attachment.marks.length > 0 && (
        <ol className="user-image-card__marks">
          {attachment.marks.map((m) => (
            <li key={m.id}>
              <strong>{m.kind === "point" ? "点" : "框"}</strong> {m.comment}
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
