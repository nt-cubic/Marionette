const SECTION_MARKER = /^§+\d*§*$/u;
const SECTION_MARKER_PREFIX = /^\s*§+\d+§*\s*/u;
/** Plain or Markdown-bold Codex/status keys, e.g. `Model:` or `**Model:**`. */
const RUNTIME_METADATA_LINE =
  /^(?:\*\*)?(?:Model|Directory|Approval|Sandbox|Account|Session|Token usage|Context window|Weekly limit|Credits|codex\s+(?:Weekly limit|Credits))(?:\*\*)?:\s*.*$/iu;
/** Codex rate-limit rows such as `**5h limit:** 42% left · resets …`. */
const RUNTIME_RATE_LIMIT_LINE =
  /^\*\*[^*\r\n]*(?:\d+\s*[mhd]|weekly)\s+limit:\*\*\s*\d+(?:\.\d+)?%\s+left\b.*$/iu;
/**
 * Claude Code `/usage` rows (local slash command; same role as Codex `/status`).
 * Live shape (claude-agent-acp): subscription banner + Current session/week %
 * used + "What's contributing…" breakdown. Parsed into the Usage panel first;
 * presentation strips the block so it never opens a hollow Reply card.
 */
const CLAUDE_USAGE_LINE =
  /^(?:You are currently using your subscription to power your Claude Code usage\s*|Current\s+(?:session|week)\b[^:\n]*:\s*\d+(?:\.\d+)?\s*%\s*used\b.*|What'?s contributing to your limits usage\??\s*|Approximate,\s*based on local sessions\b.*|Last\s+\d+\s*[hdwmy]?\s*[·•\-]\s*\d+\s+requests?\b.*|\d+(?:\.\d+)?%\s+of your usage was at\b.*)$/iu;

/**
 * Remove section markers that some ACP/OpenCode streams expose as visible
 * assistant text (for example `§11` or `§13§ Reply`). They are transport
 * delimiters, not user-facing prose.
 *
 * Only whole-line markers and markers at the beginning of a line are touched;
 * ordinary prose containing a section sign remains unchanged.
 */
export function stripSectionMarkers(text: string): string {
  if (!text || !text.includes("§")) return text;

  const cleaned = text
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .filter((line) => !SECTION_MARKER.test(line.trim()))
    .map((line) => line.replace(SECTION_MARKER_PREFIX, ""))
    .join("\n");

  return cleaned.trim() ? cleaned : "";
}

function isRuntimeMetadataLine(line: string): boolean {
  const trimmed = line.trim();
  return (
    RUNTIME_METADATA_LINE.test(trimmed) ||
    RUNTIME_RATE_LIMIT_LINE.test(trimmed) ||
    CLAUDE_USAGE_LINE.test(trimmed)
  );
}

/**
 * Hide the fixed runtime footer emitted by Codex `/status`, Claude `/usage`,
 * and similar local commands. The raw ACP text is still available to the usage
 * parser before it reaches this presentation boundary.
 *
 * A footer is removed only when every non-empty line from its first known key
 * to the end is another known metadata line. The caller should pass the
 * assembled message, not an individual streaming delta.
 */
export function stripRuntimeMetadata(text: string): string {
  if (!text) return text;

  const normalized = text.replace(/\r\n?/g, "\n");
  const lines = normalized.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    if (!isRuntimeMetadataLine(lines[index])) continue;

    const tail = lines.slice(index).filter((line) => line.trim());
    if (tail.length === 0 || !tail.every(isRuntimeMetadataLine)) continue;

    const visible = lines.slice(0, index).join("\n").replace(/[ \t\r\n]+$/g, "");
    return visible.trim() ? visible : "";
  }

  return text;
}

/**
 * True when the whole chunk is only Codex/Claude runtime status lines (no
 * user-facing prose left after strip). Used to keep `/status` and `/usage` out
 * of the chat rail while still allowing usage parsers to read the raw text.
 */
export function isRuntimeMetadataOnly(text: string): boolean {
  if (!text || !text.trim()) return false;
  const normalized = text.replace(/\r\n?/g, "\n");
  const hasMetadataLine = normalized
    .split("\n")
    .some((line) => isRuntimeMetadataLine(line));
  if (!hasMetadataLine) return false;
  return !stripRuntimeMetadata(normalized).trim();
}

/** Clean assistant-facing transport artifacts while preserving user text. */
export function cleanAssistantText(text: string): string {
  return stripRuntimeMetadata(stripSectionMarkers(text));
}

function splitTableRow(line: string): string[] | null {
  const trimmed = line.trim();
  if (!trimmed.includes("|")) return null;

  const hasLeadingPipe = trimmed.startsWith("|");
  const source = hasLeadingPipe ? trimmed.slice(1) : trimmed;
  const cells: string[] = [];
  let cell = "";
  let escaped = false;

  for (const character of source) {
    if (character === "|" && !escaped) {
      cells.push(cell);
      cell = "";
    } else {
      cell += character;
    }
    escaped = character === "\\" && !escaped;
  }
  cells.push(cell);

  if (cells.length > 1 && cells[cells.length - 1].trim() === "") {
    cells.pop();
  }
  return cells;
}

function isDelimiterCell(cell: string): boolean {
  return /^:?-{3,}:?$/.test(cell.trim());
}

function formatTableRow(original: string, cells: string[]): string {
  const trimmed = original.trim();
  const indent = original.slice(0, original.length - original.trimStart().length);
  const leading = trimmed.startsWith("|") ? "|" : "";
  const trailing = trimmed.endsWith("|") ? "|" : "";
  return `${indent}${leading}${cells.join("|")}${trailing}`;
}

type FenceState = { character: "`" | "~" | null; length: number };

function updateFenceState(line: string, state: FenceState): void {
  const match = line.match(/^\s{0,3}(`{3,}|~{3,})/);
  if (!match) return;

  const marker = match[1];
  const character = marker[0] as "`" | "~";
  if (!state.character) {
    state.character = character;
    state.length = marker.length;
    return;
  }

  if (state.character === character && marker.length >= state.length) {
    state.character = null;
    state.length = 0;
  }
}

/** Language/info string only — not mid-sentence prose after bare ```. */
const FENCE_INFO_STRING = /^\s*[\w.+#*/-]*\s*$/;

/**
 * Fix a common model Markdown mistake: fenced code glued to preceding prose.
 *
 * Example (real transcript):
 *   校验要求：```json
 *   { "type": "api" }
 *   ```
 *
 * CommonMark requires the opening fence at line start. Without a newline the
 * ticks render as literal text, the real closer becomes an opener, and the
 * rest of the Reply is swallowed into an empty code block.
 *
 * Also peels a closer that models stick to the last code line
 * (`…扔了```) onto its own line so the fence actually terminates.
 *
 * Leaves prose like `请用 ``` 包裹代码` alone (spaces + non-lang after ticks).
 */
export function normalizeMarkdownFences(text: string): string {
  if (!text || (!text.includes("```") && !text.includes("~~~"))) return text;

  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const out: string[] = [];
  const fence: FenceState = { character: null, length: 0 };

  for (let index = 0; index < lines.length; index += 1) {
    let line = lines[index];

    if (!fence.character) {
      // "prose：```json" / "prose```" → split so the fence starts its own line.
      const gluedOpen = line.match(/^(.*\S)[ \t]*(`{3,}|~{3,})([^\n`~]*)$/);
      if (gluedOpen && FENCE_INFO_STRING.test(gluedOpen[3])) {
        out.push(gluedOpen[1]);
        line = gluedOpen[2] + gluedOpen[3].trimStart();
      }
      updateFenceState(line, fence);
      out.push(line);
      continue;
    }

    // Closer stuck to the last code line (ticks at EOL): `…扔了```
    // Only EOL — mid-line ``` inside samples must stay as code body.
    const midClose = line.match(/^(.*\S)[ \t]*(`{3,}|~{3,})[ \t]*$/);
    if (
      midClose &&
      midClose[2][0] === fence.character &&
      midClose[2].length >= fence.length
    ) {
      out.push(midClose[1]);
      out.push(midClose[2]);
      fence.character = null;
      fence.length = 0;
      continue;
    }

    updateFenceState(line, fence);
    out.push(line);
  }

  return out.join("\n");
}

/**
 * Run a transform only on prose outside fenced code/tilde blocks so Chinese
 * section breaks never rewrite code samples.
 */
function mapOutsideFences(text: string, mapOutside: (segment: string) => string): string {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const fence: FenceState = { character: null, length: 0 };
  const parts: string[] = [];
  let outside = "";

  const flushOutside = () => {
    if (outside.length === 0) return;
    parts.push(mapOutside(outside));
    outside = "";
  };

  for (const line of lines) {
    const wasInFence = fence.character != null;
    updateFenceState(line, fence);
    const nowInFence = fence.character != null;

    if (wasInFence || nowInFence) {
      flushOutside();
      parts.push(line);
    } else {
      outside = outside.length > 0 ? `${outside}\n${line}` : line;
    }
  }
  flushOutside();
  return parts.join("\n");
}

/**
 * Soft-wrap section headers models stick to the previous Chinese sentence:
 * "...能力。一、技术" / "...能力。**标题" / "...。## Title"
 */
function breakGluedChineseSections(text: string): string {
  return mapOutsideFences(text, (segment) => {
    let t = segment;
    t = t.replace(/([。！？；])\s*(?=[一二三四五六七八九十]+[、．.])/g, "$1\n\n");
    t = t.replace(/([。！？；])\s*(?=#{1,6}\s)/g, "$1\n\n");
    t = t.replace(/([。！？；])\s*(?=\*\*[^*\n]{1,40}\*\*)/g, "$1\n\n");
    return t;
  });
}

/**
 * Normalize stream/model Markdown before ReactMarkdown:
 * fences, Chinese section breaks (outside code), GFM table delimiters.
 */
export function prepareMarkdownForRender(text: string): string {
  let t = text.replace(/\r\n/g, "\n");
  t = normalizeMarkdownFences(t);
  t = breakGluedChineseSections(t);
  return normalizeMarkdownTables(t);
}

/**
 * Repair a common model Markdown mistake: a table delimiter row with fewer
 * cells than its header row. GFM treats that as a paragraph, so add/trim
 * delimiter cells to the header width before ReactMarkdown sees it.
 *
 * Fenced code is deliberately left untouched because pipes there are code,
 * not table syntax.
 */
export function normalizeMarkdownTables(text: string): string {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const fence: FenceState = { character: null, length: 0 };

  for (let index = 0; index < lines.length - 1; index += 1) {
    const line = lines[index];
    updateFenceState(line, fence);
    if (fence.character) continue;

    const header = splitTableRow(line);
    const delimiter = splitTableRow(lines[index + 1]);
    if (!header || header.length < 2 || !delimiter || !delimiter.every(isDelimiterCell)) {
      continue;
    }

    if (delimiter.length !== header.length) {
      const repaired = delimiter.slice(0, header.length);
      while (repaired.length < header.length) repaired.push("---");
      lines[index + 1] = formatTableRow(lines[index + 1], repaired);
    }

    // The delimiter row is not a header candidate, so skip over it.
    index += 1;
  }

  return lines.join("\n");
}
