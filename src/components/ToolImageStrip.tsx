import { useEffect, useState } from "react";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";
import { resolveToolImagePath, type ToolImageRef } from "../lib/toolBody";

type ToolImageStripProps = {
  images: ToolImageRef[];
  /** Session working directory — relative tool paths resolve against it. */
  cwd?: string | null;
};

/**
 * Images a tool actually looked at. Read cards used to show only the path, so
 * "the model saw this screenshot" was invisible in the transcript.
 */
export function ToolImageStrip({ images, cwd }: ToolImageStripProps) {
  if (images.length === 0) return null;
  return (
    <div className="tool-images">
      {images.map((image) => (
        <ToolImageThumb
          key={image.kind === "data" ? image.label : image.path}
          image={image}
          cwd={cwd}
        />
      ))}
    </div>
  );
}

function ToolImageThumb({ image, cwd }: { image: ToolImageRef; cwd?: string | null }) {
  const [dataUrl, setDataUrl] = useState<string | null>(image.kind === "data" ? image.dataUrl : null);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    if (image.kind === "data") {
      setDataUrl(image.dataUrl);
      setError(null);
      return;
    }
    let cancelled = false;
    setDataUrl(null);
    setError(null);
    if (!isTauriRuntime()) {
      setError("preview unavailable");
      return;
    }
    void readImageDataUrl(resolveToolImagePath(image.path, cwd))
      .then((res) => {
        if (!cancelled) setDataUrl(res.dataUrl);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [image, cwd]);

  const title = image.kind === "data" ? image.label : image.path;

  return (
    <div className="tool-image-card">
      <button
        type="button"
        className="tool-image-card__frame"
        onClick={() => setExpanded((value) => !value)}
        title={title}
      >
        {dataUrl && (
          <img
            src={dataUrl}
            alt={image.label}
            className={
              expanded ? "tool-image-card__img is-expanded" : "tool-image-card__img"
            }
          />
        )}
        {!dataUrl && !error && <span className="tool-image-card__muted">…</span>}
        {error && <span className="tool-image-card__error">{image.label}</span>}
      </button>
      <span className="tool-image-card__caption" title={title}>
        {image.label}
      </span>
    </div>
  );
}
