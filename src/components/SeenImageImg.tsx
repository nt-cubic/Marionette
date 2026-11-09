import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";

const REMOTE = /^(?:https?|data|blob):/i;

/**
 * <img> for a seen image: remote / data URLs as they are; local files through the
 * asset protocol (the browser lazy-loads and discards decoded bitmaps off-screen, so
 * hundreds of thumbnails stay cheap), with the Rust reader as the fallback for a
 * refused format. Renders an empty placeholder (`is-broken`) when nothing loads.
 */
export function SeenImageImg({
  src,
  className,
  alt = "",
  eager = false,
}: {
  src: string;
  className?: string;
  alt?: string;
  /** The big picture in the viewer loads right away; thumbnails are lazy. */
  eager?: boolean;
}) {
  const remote = REMOTE.test(src);
  const [fallback, setFallback] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setFallback(null);
    setFailed(false);
  }, [src]);
  const direct = remote ? src : isTauriRuntime() ? convertFileSrc(src) : null;
  const display = fallback ?? direct;
  if (!display || failed) return <span className={`${className ?? ""} is-broken`.trim()} aria-hidden="true" />;
  return (
    <img
      src={display}
      alt={alt}
      className={className}
      loading={eager ? "eager" : "lazy"}
      draggable={false}
      onError={() => {
        if (remote || fallback || !isTauriRuntime()) {
          setFailed(true);
          return;
        }
        void readImageDataUrl(src)
          .then((result) => setFallback(result.dataUrl))
          .catch(() => setFailed(true));
      }}
    />
  );
}
