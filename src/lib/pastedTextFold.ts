/**
 * Large-paste folding for the Composer.
 *
 * A pasted block is folded into a Marionette-owned text attachment when:
 *  - the pasted UTF-8 payload itself is at least `PASTE_FOLD_BYTES` (32 KiB), or
 *  - inserting it (respecting the current selection) would push the draft past
 *    `COMPOSER_TEXT_LIMIT_BYTES`.
 *
 * Measurements are UTF-8 bytes, never JS string length, so a CJK paste folds
 * at ~10.9k characters instead of the ~32k an ASCII `length` check implies.
 *
 * Everything here is pure so `scripts/verify-pasted-text-fold.mts` can exercise
 * the thresholds, selection replacement and reference dedupe without a DOM.
 */

export const PASTE_FOLD_BYTES = 32 * 1024;

/**
 * Soft draft ceiling used only by paste folding. The textarea itself has no
 * hard `maxLength`; this is the point where keeping more text inline stops
 * being useful and a file reference is kinder to the editor and to send-time
 * payloads.
 */
export const COMPOSER_TEXT_LIMIT_BYTES = 128 * 1024;

const encoder = new TextEncoder();

export function utf8ByteLength(text: string): number {
  return encoder.encode(text).byteLength;
}

function clampIndex(value: string, index: number): number {
  if (!Number.isFinite(index)) return 0;
  return Math.min(value.length, Math.max(0, Math.trunc(index)));
}

/** Replace `[start, end)` with `insertion`; reversed selections are honoured. */
export function replaceSelection(
  value: string,
  start: number,
  end: number,
  insertion: string,
): string {
  const from = clampIndex(value, Math.min(start, end));
  const to = clampIndex(value, Math.max(start, end));
  return `${value.slice(0, from)}${insertion}${value.slice(to)}`;
}

/**
 * True when the paste must be folded instead of inserted inline.
 * `start`/`end` are the textarea selection at paste time (selection replacement
 * is part of the size calculation, not an afterthought).
 */
export function shouldFoldPastedText(
  pastedText: string,
  currentValue: string,
  start: number,
  end: number,
): boolean {
  if (!pastedText) return false;
  if (utf8ByteLength(pastedText) >= PASTE_FOLD_BYTES) return true;
  const nextInline = replaceSelection(currentValue, start, end, pastedText);
  return utf8ByteLength(nextInline) > COMPOSER_TEXT_LIMIT_BYTES;
}

/**
 * Ctrl+Shift+V / Cmd+Shift+V is the explicit inline bypass: the paste falls
 * through to the browser's default insertion, with no staging and no chip.
 */
export function isInlinePasteBypass(event: {
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}): boolean {
  return event.shiftKey && (event.ctrlKey || event.metaKey);
}

export type ReferenceInsertion = {
  value: string;
  /** Caret position just after the inserted reference (and its trailing break). */
  caret: number;
  inserted: string;
};

/**
 * Insert one reference token at the replaced selection. The token is spaced
 * away from adjacent prose the same way dropped paths are, but the selected
 * range itself is always replaced - never appended after it.
 */
export function insertReferenceAtSelection(
  value: string,
  start: number,
  end: number,
  reference: string,
): ReferenceInsertion {
  const from = clampIndex(value, Math.min(start, end));
  const to = clampIndex(value, Math.max(start, end));
  const before = value.slice(0, from);
  const after = value.slice(to);
  const needsLead = before.length > 0 && !/\s$/.test(before);
  const needsTrail = after.length > 0 && !/^\s/.test(after);
  const inserted = `${needsLead ? "\n" : ""}${reference}${needsTrail ? "\n" : ""}`;
  return {
    value: `${before}${inserted}${after}`,
    caret: before.length + inserted.length,
    inserted,
  };
}

/**
 * Where a reference for `path` already lives in the draft, as a caret position
 * (end of that line), or -1. Repeated paste events reuse the content-addressed
 * file and must not append a second chip.
 */
export function findPastedTextReferenceCaret(value: string, path: string): number {
  if (!path) return -1;
  const at = value.indexOf(path);
  if (at < 0) return -1;
  const lineEnd = value.indexOf("\n", at);
  return lineEnd >= 0 ? lineEnd : value.length;
}

/** Human size for the reference token; binary units match the 32 KiB threshold. */
export function formatByteSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}
