import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isTauriRuntime, readImageDataUrl } from "../lib/api";
import { resolveToolImagePath } from "../lib/toolBody";
import { ImageLightbox } from "./ImageLightbox";

const REMOTE_SRC = /^(?:https?|data|blob):/i;

type ToolImageThumbsProps = {
  srcs?: readonly string[] | null;
  /** Session working directory — relative paths are joined to it. */
  cwd?: string | null;
  max?: number;
  className?: string;
};

/**
 * Tiny image chips on collapsed tool / activity rows (Claude-style). The asset
 * protocol serves local files without loading their bytes into JS; the Rust
 * reader is only the fallback for a near-miss filename or refused format.
 */
export function ToolImageThumbs({
  srcs,
  cwd,
  max = 3,
  className,
}: ToolImageThumbsProps) {
  const unique: string[] = [];
  const seen = new Set<string>();
  for (const src of srcs ?? []) {
    if (!src) continue;
    const key = src.replace(/\\/g, "/").toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    unique.push(src);
    if (unique.length >= max) break;
  }
  if (unique.length === 0) return null;
  return (
    <span className={className ? `tool-thumbs ${className}` : "tool-thumbs"}>
      {unique.map((src) => (
        <ToolThumb key={src} src={src} cwd={cwd} />
      ))}
    </span>
  );
}

function ToolThumb({ src, cwd }: { src: string; cwd?: string | null }) {
  const remote = REMOTE_SRC.test(src);
  const resolved = remote ? src : resolveToolImagePath(src, cwd);
  const assetSrc =
    remote || !isTauriRuntime() ? (remote ? resolved : null) : convertFileSrc(resolved);
  const [fallback, setFallback] = useState<string | null>(null);
  const [hidden, setHidden] = useState(false);
  const [open, setOpen] = useState(false);
  const display = fallback ?? assetSrc;
  if (!display || hidden) return null;
  return (
    <>
      <button
        type="button"
        className="tool-thumb"
        title={resolved}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          setOpen(true);
        }}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <img
          src={display}
          alt=""
          loading="lazy"
          draggable={false}
          onError={() => {
            if (remote || fallback) {
              setHidden(true);
              return;
            }
            void readImageDataUrl(resolved)
              .then((result) => setFallback(result.dataUrl))
              .catch(() => setHidden(true));
          }}
        />
      </button>
      {open ? (
        <ImageLightbox
          src={display}
          alt=""
          caption={resolved}
          onClose={() => setOpen(false)}
        />
      ) : null}
    </>
  );
}
