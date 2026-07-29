/**
 * Unified annotation model (v1 = text send only).
 * Quote pins are the existing product path; range/line/image kinds land later.
 */

export type Annotation = {
  id: string;
  comment: string;
  createdAt: string;
} & (
  | { kind: "quote"; quoted: string; x: number; y: number }
  | {
      kind: "range";
      filePath: string;
      startOffset: number;
      endOffset: number;
      quoted: string;
    }
  | {
      kind: "line";
      filePath: string;
      side: "old" | "new";
      lineNumber: number;
      quoted: string;
    }
  | { kind: "point"; imagePath: string; nx: number; ny: number }
  | {
      kind: "rect";
      imagePath: string;
      nx: number;
      ny: number;
      nw: number;
      nh: number;
    }
);

export type QuotePinLike = {
  id: string;
  quoted: string;
  comment: string;
  x: number;
  y: number;
};

/** Existing QuotePin → Annotation (quote kind). */
export function quotePinToAnnotation(pin: QuotePinLike): Annotation {
  return {
    id: pin.id,
    comment: pin.comment,
    createdAt: new Date().toISOString(),
    kind: "quote",
    quoted: pin.quoted,
    x: pin.x,
    y: pin.y,
  };
}

/**
 * Render annotations + free text for `send_prompt` (plain text only).
 * Same spirit as `formatPinsForSend`.
 */
export function formatAnnotationsForSend(
  annotations: Annotation[],
  freeText: string,
): string {
  const blocks = annotations.map((a, index) => {
    const n = index + 1;
    const comment = a.comment.replace(/\r\n/g, "\n").trim();
    if (a.kind === "quote") {
      const quoted = a.quoted.replace(/\r\n/g, "\n").trim();
      const lines = quoted.split("\n").map((line) => `> ${line}`);
      return `${n}.\n${lines.join("\n")}\n评论：${comment}`;
    }
    if (a.kind === "range") {
      const quoted = a.quoted.replace(/\r\n/g, "\n").trim();
      const lines = quoted.split("\n").map((line) => `> ${line}`);
      return `${n}. ${a.filePath}\n${lines.join("\n")}\n评论：${comment}`;
    }
    if (a.kind === "line") {
      const sideLabel = a.side === "new" ? "新" : "旧";
      const quoted = a.quoted.replace(/\r\n/g, "\n").trim();
      const lines = quoted
        ? quoted.split("\n").map((line) => `> ${line}`)
        : [];
      const head = `${n}. ${a.filePath}:${a.lineNumber}（${sideLabel}）`;
      return lines.length
        ? `${head}\n${lines.join("\n")}\n评论：${comment}`
        : `${head}\n评论：${comment}`;
    }
    if (a.kind === "point") {
      return `${n}. ${a.imagePath} @ (${a.nx.toFixed(3)}, ${a.ny.toFixed(3)})\n评论：${comment}`;
    }
    // rect
    return `${n}. ${a.imagePath} rect (${a.nx.toFixed(3)}, ${a.ny.toFixed(3)}, ${a.nw.toFixed(3)}×${a.nh.toFixed(3)})\n评论：${comment}`;
  });
  const free = freeText.replace(/\r\n/g, "\n").trim();
  if (free) blocks.push(free);
  return blocks.join("\n\n");
}

export type DiffLine = {
  raw: string;
  type: "add" | "del" | "ctx" | "hunk" | "meta";
  oldLine: number | null;
  newLine: number | null;
};

/**
 * Parse unified diff text into lines with both-side line numbers.
 * Handles `@@ -a,b +c,d @@` hunk headers.
 */
export function parseUnifiedDiff(text: string): DiffLine[] {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const out: DiffLine[] = [];
  let oldLine = 0;
  let newLine = 0;
  let inHunk = false;

  for (const raw of lines) {
    if (raw.startsWith("@@")) {
      const m = raw.match(/@@\s+-(\d+)(?:,\d+)?\s+\+(\d+)(?:,\d+)?\s*@@/);
      if (m) {
        oldLine = Number(m[1]);
        newLine = Number(m[2]);
        inHunk = true;
        out.push({ raw, type: "hunk", oldLine: null, newLine: null });
        continue;
      }
    }
    if (
      raw.startsWith("diff ") ||
      raw.startsWith("index ") ||
      raw.startsWith("--- ") ||
      raw.startsWith("+++ ") ||
      raw.startsWith("new file") ||
      raw.startsWith("deleted file") ||
      raw.startsWith("similarity ") ||
      raw.startsWith("rename ")
    ) {
      inHunk = false;
      out.push({ raw, type: "meta", oldLine: null, newLine: null });
      continue;
    }
    if (!inHunk) {
      out.push({ raw, type: "meta", oldLine: null, newLine: null });
      continue;
    }
    if (raw.startsWith("+") && !raw.startsWith("+++")) {
      out.push({ raw, type: "add", oldLine: null, newLine });
      newLine += 1;
      continue;
    }
    if (raw.startsWith("-") && !raw.startsWith("---")) {
      out.push({ raw, type: "del", oldLine, newLine: null });
      oldLine += 1;
      continue;
    }
    // context (leading space or empty)
    out.push({
      raw,
      type: "ctx",
      oldLine,
      newLine,
    });
    oldLine += 1;
    newLine += 1;
  }
  return out;
}
