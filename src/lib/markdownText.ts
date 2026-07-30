const SECTION_MARKER = /^§+\d*§*$/u;
const SECTION_MARKER_PREFIX = /^\s*§+\d+§*\s*/u;
/** Plain or Markdown-bold Codex/status keys, e.g. `Model:` or `**Model:**`. */
const RUNTIME_METADATA_LINE =
  /^(?:\*\*)?(?:Model|Directory|Approval|Sandbox|Account|Session|Token usage|Context window|Weekly limit|Credits|codex\s+(?:Weekly limit|Credits))(?:\*\*)?:\s*.*$/iu;
/** Codex rate-limit rows such as `**5h limit:** 42% left · resets …`. */
const RUNTIME_RATE_LIMIT_LINE =
  /^\*\*[^*\r\n]*(?:\d+\s*[mhd]|weekly)\s+limit:\*\*\s*\d+(?:\.\d+)?%\s+left\b.*$/iu;

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
  return RUNTIME_METADATA_LINE.test(trimmed) || RUNTIME_RATE_LIMIT_LINE.test(trimmed);
}

/**
 * Hide the fixed runtime footer emitted by Codex `/status` and similar local
 * commands. The raw ACP text is still available to the usage parser before it
 * reaches this presentation boundary.
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
 * True when the whole chunk is only Codex/runtime status lines (no user-facing
 * prose left after strip). Used to keep `/status` out of the chat rail while
 * still allowing usage parsers to read the raw text.
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

function updateFenceState(
  line: string,
  state: { character: "`" | "~" | null; length: number },
): void {
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
  const fence = { character: null as "`" | "~" | null, length: 0 };

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
