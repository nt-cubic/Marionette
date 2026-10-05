/** Image attachments + annotations for Composer pills and You cards. */

export type ImagePointAnnotation = {
  id: string;
  kind: "point";
  /** Normalized 0–1 relative to natural image size. */
  nx: number;
  ny: number;
  comment: string;
};

export type ImageRectAnnotation = {
  id: string;
  kind: "rect";
  nx: number;
  ny: number;
  nw: number;
  nh: number;
  comment: string;
};

export type ImageMark = ImagePointAnnotation | ImageRectAnnotation;

export type ImageAttachment = {
  id: string;
  path: string;
  name: string;
  mimeType: string;
  marks: ImageMark[];
};

const IMAGE_EXT = new Set([
  ".png",
  ".jpg",
  ".jpeg",
  ".gif",
  ".webp",
  ".bmp",
  ".svg",
]);

export function isImagePath(path: string): boolean {
  const base = stripFileUri(path).replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? path;
  const dot = base.lastIndexOf(".");
  if (dot < 0) return false;
  return IMAGE_EXT.has(base.slice(dot).toLowerCase());
}

/** `file:///D:/x.png` / `file://localhost/C:/x` → filesystem path. */
export function stripFileUri(value: string): string {
  const trimmed = value.trim();
  if (!/^file:/i.test(trimmed)) return trimmed;
  const fromUrl = (url: URL): string => {
    let rest = decodeURIComponent(url.pathname);
    if (url.hostname && url.hostname.toLowerCase() !== "localhost") {
      rest = `//${url.hostname}${rest}`;
    }
    if (/^\/[a-zA-Z]:/.test(rest)) rest = rest.slice(1);
    else if (/^\/[a-zA-Z]\|/.test(rest)) rest = `${rest.charAt(1)}:${rest.slice(3)}`;
    return rest;
  };
  try {
    return fromUrl(new URL(trimmed));
  } catch {
    // Non-standard `file://D:/x` (two slashes + drive) — strip manually.
    let rest = trimmed.replace(/^file:\/\//i, "").replace(/^file:/i, "");
    if (/^\/[a-zA-Z]:/.test(rest)) rest = rest.slice(1);
    try {
      rest = decodeURIComponent(rest);
    } catch {
      // keep the raw remainder
    }
    return rest;
  }
}

function pathFromToolInputJson(input: string): string | undefined {
  const trimmed = input.trim();
  if (!trimmed) return undefined;
  if (trimmed.startsWith("{")) {
    try {
      const parsed = JSON.parse(trimmed) as Record<string, unknown>;
      for (const key of ["path", "filePath", "file_path", "uri"]) {
        const value = parsed[key];
        if (typeof value === "string" && value.trim()) return value.trim();
      }
    } catch {
      return undefined;
    }
    return undefined;
  }
  // Plain string: a real path (`D:\a.png`, `screenshot.png`), not `ls foo.png`.
  if (!isImagePath(trimmed)) return undefined;
  const base = trimmed.replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? trimmed;
  if (base !== trimmed) return trimmed;
  return /\s/.test(trimmed) ? undefined : trimmed;
}

/**
 * Image srcs a tool card can preview: ACP `images[]`, `locations` path, and
 * structured `rawInput` path fields. Never treats a shell command as a path.
 */
export function collectToolImageSrcs(event: {
  path?: string;
  input?: string;
  images?: string[];
}): string[] {
  const srcs: string[] = [];
  const add = (raw?: string) => {
    if (!raw) return;
    const src = stripFileUri(raw);
    if (!src || src.startsWith("data:")) return;
    const http = /^https?:\/\//i.test(src);
    if (!http && !isImagePath(src)) return;
    if (srcs.some((existing) => existing.replace(/\\/g, "/").toLowerCase() === src.replace(/\\/g, "/").toLowerCase())) {
      return;
    }
    srcs.push(src);
  };
  if (Array.isArray(event.images)) {
    for (const src of event.images) add(src);
  }
  add(event.path);
  add(pathFromToolInputJson(event.input ?? ""));
  return srcs;
}

export function mimeFromImagePath(path: string): string {
  const lower = path.toLowerCase();
  if (lower.endsWith(".png")) return "image/png";
  if (lower.endsWith(".jpg") || lower.endsWith(".jpeg")) return "image/jpeg";
  if (lower.endsWith(".gif")) return "image/gif";
  if (lower.endsWith(".webp")) return "image/webp";
  if (lower.endsWith(".bmp")) return "image/bmp";
  if (lower.endsWith(".svg")) return "image/svg+xml";
  return "application/octet-stream";
}

export function fileNameFromPath(path: string): string {
  return path.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || path;
}

export function newAttachmentId(): string {
  return `img-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}

export function newMarkId(): string {
  return `mk-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}

export function attachmentFromPath(path: string): ImageAttachment {
  return {
    id: newAttachmentId(),
    path,
    name: fileNameFromPath(path),
    mimeType: mimeFromImagePath(path),
    marks: [],
  };
}

/** Text block for session/prompt describing marks on one or more images. */
export function formatImageMarksForSend(attachments: ImageAttachment[]): string {
  const blocks: string[] = [];
  let n = 0;
  for (const att of attachments) {
    if (att.marks.length === 0) continue;
    for (const m of att.marks) {
      n += 1;
      if (m.kind === "point") {
        blocks.push(
          `${n}. ${att.name} @ (${m.nx.toFixed(3)}, ${m.ny.toFixed(3)})\n评论：${m.comment.trim()}`,
        );
      } else {
        blocks.push(
          `${n}. ${att.name} rect (${m.nx.toFixed(3)}, ${m.ny.toFixed(3)}, ${m.nw.toFixed(3)}×${m.nh.toFixed(3)})\n评论：${m.comment.trim()}`,
        );
      }
    }
  }
  return blocks.join("\n\n");
}

export function clamp01(n: number): number {
  if (!Number.isFinite(n)) return 0;
  return Math.min(1, Math.max(0, n));
}
