/**
 * P1-9 - versioned prompt links.
 *
 * A prompt link is a deep link that can prefill a *draft*:
 *
 *   marionette://new?v=1&prompt=...&project=...&session=...&agent=...&model=...
 *
 * Security contract (do not weaken without re-reading P1-9):
 * - Only the five fields above are ever written. Permissions, approval modes
 *   and auto-send have no representation in this type and are never emitted.
 * - Inbound permission/auto-send parameters are dropped and reported.
 * - Control characters, bidi overrides, tag characters and other invisible
 *   formatting characters are removed; the removal count is reported.
 * - Prompt text is capped. Truncation is reported.
 * - Nothing here sends a prompt, changes a permission, or touches the DOM.
 *   Callers may prefill a draft; sending remains an explicit user action.
 *
 * The parser is deliberately strict about the version but tolerant about
 * unknown *safe* parameters (they are ignored and reported). This lets newer
 * producers add fields without breaking older clients.
 */

export const PROMPT_LINK_VERSION = 1;
export const PROMPT_LINK_SCHEME = "marionette";
export const PROMPT_LINK_HOST = "new";
export const PROMPT_LINK_MAX_PROMPT_CHARS = 8000;
export const PROMPT_LINK_MAX_ID_CHARS = 200;
export const PROMPT_LINK_MAX_PATH_CHARS = 4096;

/** Canonical parameter names. Aliases are intentionally not accepted. */
export const PROMPT_LINK_FIELDS = [
  "prompt",
  "project",
  "session",
  "agent",
  "model",
] as const;

export type PromptLinkField = (typeof PROMPT_LINK_FIELDS)[number];

/**
 * Parameters that could widen authority or start a turn. They are never
 * serialized, and an inbound link carrying them has them removed.
 * Lower-case comparison; unknown casing is still caught.
 */
export const PROMPT_LINK_FORBIDDEN_PARAMS: ReadonlySet<string> = new Set([
  "permission",
  "permissions",
  "permissionmode",
  "permission_mode",
  "mode",
  "alwaysapprove",
  "always_approve",
  "autosend",
  "auto_send",
  "send",
  "submit",
  "autostart",
  "auto_start",
  "run",
  "execute",
  "approve",
  "approval",
  "bypass",
  "force",
]);

export type PromptLink = {
  version: typeof PROMPT_LINK_VERSION;
  /** Sanitized draft text. May be empty when the link only selects scope. */
  prompt: string;
  project?: string;
  session?: string;
  agent?: string;
  model?: string;
};

export type PromptLinkInput = {
  prompt?: string | null;
  project?: string | null;
  session?: string | null;
  agent?: string | null;
  model?: string | null;
};

export type PromptLinkIssue =
  | { code: "control-removed"; field: PromptLinkField; count: number }
  | { code: "hidden-removed"; field: PromptLinkField; count: number }
  | { code: "linebreaks-normalized"; field: "prompt"; count: number }
  | { code: "truncated"; field: PromptLinkField; count: number }
  | {
      code: "field-dropped";
      field: PromptLinkField;
      reason: "empty" | "invalid" | "too-long";
    }
  | {
      code: "param-dropped";
      param: string;
      reason: "forbidden" | "unknown" | "duplicate" | "invalid";
    }
  | { code: "version-unsupported"; value: string }
  | { code: "malformed"; detail: string };

export type PromptLinkReport = {
  /** control + hidden + truncated; the single number most UIs want to show. */
  removedChars: number;
  controlRemovedChars: number;
  hiddenRemovedChars: number;
  truncatedChars: number;
  normalizedLineBreaks: number;
  /** Canonical (lower-case) parameter names that were ignored. */
  droppedParams: string[];
  issues: PromptLinkIssue[];
  /** True when at least one issue exists (removal, truncation, dropped param). */
  changed: boolean;
};

export type PromptLinkSanitizeResult = {
  link: PromptLink;
  report: PromptLinkReport;
};

export type PromptLinkBuildResult = PromptLinkSanitizeResult & {
  /** Wire form. Only whitelisted fields; safe to copy to the clipboard. */
  url: string;
};

export type PromptLinkParseFailureReason =
  | "not-prompt-link"
  | "malformed"
  | "missing-version"
  | "unsupported-version"
  | "empty-link";

export type PromptLinkParseResult =
  | { ok: true; link: PromptLink; report: PromptLinkReport }
  | {
      ok: false;
      reason: PromptLinkParseFailureReason;
      report: PromptLinkReport;
    };

// ---------------------------------------------------------------------------
// Character policy
// ---------------------------------------------------------------------------

/**
 * Invisible characters used to hide text or reorder it visually. ZWJ/ZWNJ
 * (U+200C/U+200D) are intentionally *not* here: they are meaningful joiners in
 * Arabic/Persian text and emoji sequences, and they cannot reorder or hide a
 * whole instruction the way the ranges below can.
 */
const HIDDEN_CODEPOINT_RANGES: ReadonlyArray<readonly [number, number]> = [
  [0x00ad, 0x00ad], // soft hyphen
  [0x061c, 0x061c], // Arabic letter mark
  [0x115f, 0x1160], // Hangul fillers
  [0x17b4, 0x17b5], // Khmer inherent vowels
  [0x180b, 0x180f], // Mongolian free variation selectors / separator
  [0x200b, 0x200b], // zero width space
  [0x200e, 0x200f], // LRM / RLM
  [0x202a, 0x202e], // bidi embedding / override
  [0x2060, 0x206f], // word joiner, invisible operators, bidi isolates
  [0x3164, 0x3164], // Hangul filler
  [0xfeff, 0xfeff], // BOM / zero width no-break space
  [0xffa0, 0xffa0], // halfwidth Hangul filler
  [0xfff0, 0xfffb], // interlinear annotation / object replacement controls
  [0x1bca0, 0x1bca3], // shorthand format controls
  [0x1d173, 0x1d17a], // musical format controls
  [0xe0000, 0xe007f], // Unicode tags (instruction-smuggling classic)
];

function isHiddenCodePoint(cp: number): boolean {
  for (const [low, high] of HIDDEN_CODEPOINT_RANGES) {
    if (cp >= low && cp <= high) return true;
  }
  return false;
}

function isControlCodePoint(cp: number): boolean {
  return cp < 0x20 || (cp >= 0x7f && cp <= 0x9f);
}

type CleanedText = {
  text: string;
  controlRemoved: number;
  hiddenRemoved: number;
  linebreaksNormalized: number;
};

function cleanText(raw: string, preserveLineBreaks: boolean): CleanedText {
  let normalized = raw;
  let linebreaks = 0;
  if (normalized.includes("\r")) {
    const before = normalized.length;
    normalized = normalized.replace(/\r\n?/g, "\n");
    linebreaks += before - normalized.length;
  }

  let text = "";
  let control = 0;
  let hidden = 0;
  for (const char of normalized) {
    const cp = char.codePointAt(0) ?? 0;
    if (cp === 0x0a) {
      if (preserveLineBreaks) text += "\n";
      else {
        text += " ";
        control += 1;
      }
      continue;
    }
    if (cp === 0x09) {
      if (preserveLineBreaks) text += "\t";
      else {
        text += " ";
        control += 1;
      }
      continue;
    }
    if (cp === 0x2028 || cp === 0x2029) {
      text += preserveLineBreaks ? "\n" : " ";
      linebreaks += 1;
      continue;
    }
    // A lone surrogate cannot be encoded safely and is never visible.
    if (cp >= 0xd800 && cp <= 0xdfff) {
      hidden += 1;
      continue;
    }
    if (isHiddenCodePoint(cp)) {
      hidden += 1;
      continue;
    }
    if (isControlCodePoint(cp)) {
      control += 1;
      continue;
    }
    text += char;
  }

  return {
    text,
    controlRemoved: control,
    hiddenRemoved: hidden,
    linebreaksNormalized: linebreaks,
  };
}

function truncateCodePoints(
  text: string,
  max: number,
): { text: string; removed: number } {
  let kept = 0;
  let end = 0;
  let total = 0;
  for (const char of text) {
    total += 1;
    if (kept < max) {
      kept += 1;
      end += char.length;
    }
  }
  if (kept >= total) return { text, removed: 0 };
  return { text: text.slice(0, end), removed: total - kept };
}

// ---------------------------------------------------------------------------
// Report plumbing
// ---------------------------------------------------------------------------

type ReportBuilder = {
  controlRemovedChars: number;
  hiddenRemovedChars: number;
  truncatedChars: number;
  normalizedLineBreaks: number;
  droppedParams: string[];
  issues: PromptLinkIssue[];
};

function newReport(): ReportBuilder {
  return {
    controlRemovedChars: 0,
    hiddenRemovedChars: 0,
    truncatedChars: 0,
    normalizedLineBreaks: 0,
    droppedParams: [],
    issues: [],
  };
}

function finishReport(report: ReportBuilder): PromptLinkReport {
  return {
    removedChars:
      report.controlRemovedChars +
      report.hiddenRemovedChars +
      report.truncatedChars,
    controlRemovedChars: report.controlRemovedChars,
    hiddenRemovedChars: report.hiddenRemovedChars,
    truncatedChars: report.truncatedChars,
    normalizedLineBreaks: report.normalizedLineBreaks,
    droppedParams: [...report.droppedParams],
    issues: [...report.issues],
    changed: report.issues.length > 0,
  };
}

function noteCleaning(
  report: ReportBuilder,
  field: PromptLinkField,
  cleaned: CleanedText,
): void {
  if (cleaned.controlRemoved > 0) {
    report.controlRemovedChars += cleaned.controlRemoved;
    report.issues.push({
      code: "control-removed",
      field,
      count: cleaned.controlRemoved,
    });
  }
  if (cleaned.hiddenRemoved > 0) {
    report.hiddenRemovedChars += cleaned.hiddenRemoved;
    report.issues.push({
      code: "hidden-removed",
      field,
      count: cleaned.hiddenRemoved,
    });
  }
  if (field === "prompt" && cleaned.linebreaksNormalized > 0) {
    report.normalizedLineBreaks += cleaned.linebreaksNormalized;
    report.issues.push({
      code: "linebreaks-normalized",
      field: "prompt",
      count: cleaned.linebreaksNormalized,
    });
  }
}

function noteTruncation(
  report: ReportBuilder,
  field: PromptLinkField,
  removed: number,
): void {
  if (removed <= 0) return;
  report.truncatedChars += removed;
  report.issues.push({ code: "truncated", field, count: removed });
}

function dropParam(
  report: ReportBuilder,
  param: string,
  reason: "forbidden" | "unknown" | "duplicate" | "invalid",
): void {
  const canonical = param.toLowerCase();
  if (!report.droppedParams.includes(canonical)) {
    report.droppedParams.push(canonical);
  }
  report.issues.push({ code: "param-dropped", param: canonical, reason });
}

// ---------------------------------------------------------------------------
// Sanitize / serialize
// ---------------------------------------------------------------------------

function sanitizePrompt(raw: string, report: ReportBuilder): string {
  const cleaned = cleanText(raw, true);
  noteCleaning(report, "prompt", cleaned);
  const trimmed = cleaned.text.trim();
  if (!trimmed) return "";
  const capped = truncateCodePoints(trimmed, PROMPT_LINK_MAX_PROMPT_CHARS);
  noteTruncation(report, "prompt", capped.removed);
  return capped.text;
}

function sanitizeOpaqueField(
  raw: string,
  field: Exclude<PromptLinkField, "prompt">,
  report: ReportBuilder,
): string | undefined {
  const max =
    field === "project"
      ? PROMPT_LINK_MAX_PATH_CHARS
      : PROMPT_LINK_MAX_ID_CHARS;
  const cleaned = cleanText(raw, false);
  noteCleaning(report, field, cleaned);
  let value = cleaned.text.trim();
  // An absent/blank optional field is normal, not a removal to warn about.
  if (!value) return undefined;
  // Project paths may contain spaces. Identifiers may not: whitespace in an id
  // is either a typo or an attempt to smuggle a second token.
  if (field !== "project" && /\s/.test(value)) {
    report.issues.push({ code: "field-dropped", field, reason: "invalid" });
    return undefined;
  }
  const capped = truncateCodePoints(value, max);
  if (capped.removed > 0) {
    value = capped.text;
    noteTruncation(report, field, capped.removed);
  }
  return value;
}

function asString(value: string | null | undefined): string {
  return typeof value === "string" ? value : "";
}

/**
 * Sanitize an in-memory link payload (for example, one received as JSON).
 * Empty optional fields are omitted from the result.
 */
export function sanitizePromptLink(
  input: PromptLinkInput,
): PromptLinkSanitizeResult {
  const report = newReport();
  const prompt = sanitizePrompt(asString(input.prompt), report);
  const project = sanitizeOpaqueField(
    asString(input.project),
    "project",
    report,
  );
  const session = sanitizeOpaqueField(
    asString(input.session),
    "session",
    report,
  );
  const agent = sanitizeOpaqueField(asString(input.agent), "agent", report);
  const model = sanitizeOpaqueField(asString(input.model), "model", report);

  const link: PromptLink = { version: PROMPT_LINK_VERSION, prompt };
  if (project) link.project = project;
  if (session) link.session = session;
  if (agent) link.agent = agent;
  if (model) link.model = model;

  return { link, report: finishReport(report) };
}

function encodeLink(link: PromptLink): string {
  const params = new URLSearchParams();
  params.set("v", String(PROMPT_LINK_VERSION));
  if (link.prompt) params.set("prompt", link.prompt);
  if (link.project) params.set("project", link.project);
  if (link.session) params.set("session", link.session);
  if (link.agent) params.set("agent", link.agent);
  if (link.model) params.set("model", link.model);
  return `${PROMPT_LINK_SCHEME}://${PROMPT_LINK_HOST}?${params.toString()}`;
}

/**
 * Sanitize and serialize. The returned `url` never contains permission
 * parameters or an auto-send flag - those fields do not exist in the type.
 */
export function serializePromptLink(
  input: PromptLinkInput,
): PromptLinkBuildResult {
  const sanitized = sanitizePromptLink(input);
  return { ...sanitized, url: encodeLink(sanitized.link) };
}

/** True when `raw` looks like a prompt link (used by the launch adapter). */
export function isPromptLinkUrl(raw: string): boolean {
  return /^marionette:(?:\/\/)?(?:new|prompt)(?:[/?#]|$)/i.test(raw.trim());
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

function failed(
  reason: PromptLinkParseFailureReason,
  report: ReportBuilder,
): PromptLinkParseResult {
  return { ok: false, reason, report: finishReport(report) };
}

export function parsePromptLink(raw: string): PromptLinkParseResult {
  const report = newReport();
  if (typeof raw !== "string" || !raw.trim()) {
    return failed("not-prompt-link", report);
  }
  const trimmed = raw.trim();
  if (!isPromptLinkUrl(trimmed)) return failed("not-prompt-link", report);

  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    report.issues.push({ code: "malformed", detail: "unparseable-url" });
    return failed("malformed", report);
  }

  const target = (
    url.hostname ||
    url.pathname.split("/").find((part) => Boolean(part)) ||
    ""
  ).toLowerCase();
  if (target !== "new" && target !== "prompt") {
    return failed("not-prompt-link", report);
  }

  const versions = url.searchParams.getAll("v");
  const version = versions[0];
  if (version === undefined) {
    report.issues.push({ code: "malformed", detail: "missing-v" });
    return failed("missing-version", report);
  }
  if (versions.length > 1) dropParam(report, "v", "duplicate");
  if (version !== String(PROMPT_LINK_VERSION)) {
    report.issues.push({ code: "version-unsupported", value: version });
    return failed("unsupported-version", report);
  }

  const recognized = new Set<string>(PROMPT_LINK_FIELDS);
  const values = new Map<string, string>();
  for (const [key, value] of url.searchParams) {
    const canonical = key.toLowerCase();
    if (canonical === "v") continue;
    if (PROMPT_LINK_FORBIDDEN_PARAMS.has(canonical)) {
      dropParam(report, key, "forbidden");
      continue;
    }
    if (!recognized.has(canonical)) {
      dropParam(report, key, "unknown");
      continue;
    }
    if (values.has(canonical)) {
      dropParam(report, key, "duplicate");
      continue;
    }
    values.set(canonical, value);
  }

  const prompt = sanitizePrompt(values.get("prompt") ?? "", report);
  const project = sanitizeOpaqueField(
    values.get("project") ?? "",
    "project",
    report,
  );
  const session = sanitizeOpaqueField(
    values.get("session") ?? "",
    "session",
    report,
  );
  const agent = sanitizeOpaqueField(values.get("agent") ?? "", "agent", report);
  const model = sanitizeOpaqueField(values.get("model") ?? "", "model", report);

  if (!prompt && !project && !session && !agent && !model) {
    return failed("empty-link", report);
  }

  const link: PromptLink = { version: PROMPT_LINK_VERSION, prompt };
  if (project) link.project = project;
  if (session) link.session = session;
  if (agent) link.agent = agent;
  if (model) link.model = model;

  return { ok: true, link, report: finishReport(report) };
}

/**
 * Draft-replacement guard for the caller's UI. A link may prefill an empty
 * draft silently; a non-empty draft needs an explicit confirmation first.
 * Attachments are a separate draft surface and should be checked by the caller.
 */
export function promptLinkNeedsDraftReplaceConfirmation(
  currentDraft: string,
): boolean {
  return currentDraft.trim().length > 0;
}
