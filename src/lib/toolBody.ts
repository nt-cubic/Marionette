/**
 * Tool-card body helpers (pure data, no UI).
 *
 * A tool result reaches the card as one text blob (`title · status`, path,
 * detail). Two things in that blob are not prose and deserve their own
 * rendering:
 *
 * - images the agent looked at (ACP image blocks arrive as `![image](path)`,
 *   or the read tool just prints the path) — the card used to show the path only;
 * - a unified diff an edit tool produced — it needs line numbers and colors,
 *   not a wrapped text dump.
 */

const IMAGE_EXTENSION = /\.(?:png|jpe?g|gif|webp|bmp|avif|svg)$/i;
/** `![alt](target)` — how ACP image blocks are stringified (see acpTranscript). */
const MARKDOWN_IMAGE = /!\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)/g;
/** A line that is nothing but one image markdown (surrounding spaces allowed). */
const IMAGE_ONLY_LINE = /^\s*!\[[^\]]*\]\(\s*<?[^)\s>]+>?(?:\s+"[^"]*")?\s*\)\s*$/;
/** Absolute (drive / UNC / POSIX) or explicitly relative path. */
const PATH_LIKE_LINE = /^(?:[A-Za-z]:[\\/]|\\\\|\/|\.{1,2}[\\/])/;
/** First line of a unified diff, as produced by git and by acpTranscript. */
const DIFF_START_LINE = /^(?:diff --git |diff -|Index: |--- |\+\+\+ |@@ )/;

export type ToolImageRef =
  | { kind: "data"; dataUrl: string; label: string }
  | { kind: "path"; path: string; label: string };

export type ToolBodyParts = {
  /** Body text minus the image markdown / image path lines and the diff. */
  text: string;
  images: ToolImageRef[];
  /** Unified diff text (empty when the tool produced none). */
  diff: string;
};

export function isImagePath(value: string): boolean {
  return IMAGE_EXTENSION.test(value.trim());
}

/** `file:///C:/x.png` → `C:/x.png`; other schemes pass through untouched. */
export function stripFileUrl(value: string): string {
  const withoutScheme = value.replace(/^file:\/\//i, "");
  // `file:///D:/x.png` arrives as `/D:/x.png` — drop the extra slash before a drive.
  return withoutScheme.replace(/^\/([A-Za-z]:[\\/])/, "$1");
}

/**
 * Make a tool-reported image path readable by the Rust loader, which needs an
 * absolute existing file. Relative paths resolve against the session cwd.
 */
export function resolveToolImagePath(path: string, cwd?: string | null): string {
  const value = stripFileUrl(path).trim();
  if (!value) return value;
  if (/^[A-Za-z]:[\\/]/.test(value) || value.startsWith("\\\\") || value.startsWith("/")) {
    return value;
  }
  const base = (cwd ?? "").trim();
  if (!base) return value;
  const separator = base.includes("\\") ? "\\" : "/";
  const rest = value.replace(/^\.?[\\/]+/, "");
  // Match the cwd's separator so the path stays readable in the card caption.
  const normalized =
    separator === "\\" ? rest.replace(/\//g, "\\") : rest.replace(/\\/g, "/");
  return `${base.replace(/[\\/]+$/, "")}${separator}${normalized}`;
}

function imageLabel(target: string): string {
  const clean = stripFileUrl(target).replace(/[\\/]+$/, "");
  return clean.split(/[\\/]/).filter(Boolean).pop() || clean || "image";
}

/** `data:image/png;base64,…` → `image.png` — a base64 blob has no useful tail. */
function dataImageLabel(dataUrl: string): string {
  const mime = dataUrl.match(/^data:(image\/[\w.+-]+)/i)?.[1] ?? "";
  const subtype = mime.split("/")[1]?.replace(/^svg\+xml$/i, "svg").replace(/^jpeg$/i, "jpg");
  return subtype ? `image.${subtype}` : "image";
}

function imageRef(target: string): ToolImageRef | null {
  const value = target.trim();
  if (!value) return null;
  if (/^data:image\//i.test(value)) {
    return { kind: "data", dataUrl: value, label: dataImageLabel(value) };
  }
  const path = stripFileUrl(value);
  if (!isImagePath(path)) return null;
  return { kind: "path", path, label: imageLabel(path) };
}

function unquote(value: string): string {
  const trimmed = value.trim();
  if (trimmed.length < 2) return trimmed;
  const first = trimmed[0];
  const last = trimmed[trimmed.length - 1];
  if ((first === "`" && last === "`") || (first === '"' && last === '"') || (first === "'" && last === "'")) {
    return trimmed.slice(1, -1).trim();
  }
  return trimmed;
}

/**
 * True when `diff` is a unified diff the line renderer can number.
 * Mirrors SessionView's `hasUnifiedDiff`, kept here so the split is testable.
 */
export function looksLikeUnifiedDiff(text: string): boolean {
  return /^@@\s+-\d+/m.test(text) && /^(?:\+\+\+|---|[+-])\s?/m.test(text);
}

/**
 * Split a tool body into prose, images and diff.
 *
 * `toolPath` is the tool's `locations[0].path`; when it is an image the card
 * shows that image instead of repeating the path line.
 */
export function splitToolBody(
  body: string,
  options: { toolPath?: string | null; cwd?: string | null } = {},
): ToolBodyParts {
  const toolPath = (options.toolPath ?? "").trim();
  const images: ToolImageRef[] = [];
  const seen = new Set<string>();
  const push = (ref: ToolImageRef | null) => {
    if (!ref) return;
    const key =
      ref.kind === "data" ? ref.dataUrl : ref.path.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
    if (seen.has(key)) return;
    seen.add(key);
    images.push(ref);
  };

  if (toolPath && isImagePath(toolPath)) push(imageRef(toolPath));

  const kept: string[] = [];
  for (const rawLine of (body ?? "").replace(/\r\n?/g, "\n").split("\n")) {
    if (IMAGE_ONLY_LINE.test(rawLine)) {
      for (const match of rawLine.matchAll(MARKDOWN_IMAGE)) push(imageRef(match[1] ?? ""));
      continue;
    }
    // A read tool often prints the bare path it looked at — the card renders
    // that as the image itself, so the raw line would only be noise.
    const candidate = unquote(rawLine);
    const pathLike =
      PATH_LIKE_LINE.test(candidate) || (!/\s/.test(candidate) && /[\\/]/.test(candidate));
    if (candidate && pathLike && isImagePath(candidate)) {
      const ref = imageRef(candidate);
      if (ref) {
        push(ref);
        continue;
      }
    }
    kept.push(rawLine);
  }

  let text = kept.join("\n");
  let diff = "";
  const lines = text.split("\n");
  const start = lines.findIndex((line) => DIFF_START_LINE.test(line));
  if (start >= 0) {
    const candidate = lines.slice(start).join("\n").trim();
    if (looksLikeUnifiedDiff(candidate)) {
      diff = candidate;
      text = lines.slice(0, start).join("\n");
    }
  }

  return { text: text.replace(/\s+$/, ""), images: dedupeInlineCopy(images), diff };
}

/**
 * ACP sends the image as `uri` *or* `data`, so a body carrying base64 next to
 * the tool's own path is one picture, not two — keep the inline copy.
 */
function dedupeInlineCopy(images: ToolImageRef[]): ToolImageRef[] {
  if (images.length !== 2) return images;
  const inline = images.find((image) => image.kind === "data");
  const fromPath = images.find((image) => image.kind === "path");
  if (!inline || !fromPath) return images;
  return [inline];
}

/** File a diff belongs to: the `+++ b/<path>` header, else the tool path. */
export function diffTargetPath(diff: string, fallback?: string | null): string {
  const header = diff.match(/^\+\+\+\s+([^\t\n]+)/m);
  if (header) {
    const value = header[1].trim().replace(/^[ab]\//, "");
    if (value && value !== "/dev/null") return value;
  }
  return (fallback ?? "").trim();
}
