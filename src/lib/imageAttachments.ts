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
  const base = path.replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? path;
  const dot = base.lastIndexOf(".");
  if (dot < 0) return false;
  return IMAGE_EXT.has(base.slice(dot).toLowerCase());
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
