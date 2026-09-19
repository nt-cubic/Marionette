/**
 * Legacy display helpers for the retired Composer "force web search" toggle.
 *
 * The toggle prepended a hard "search first" instruction to the wire prompt.
 * It is gone, but transcripts written while it existed still carry that block:
 * You cards must keep showing clean text, and edit & resend must keep stripping
 * it so a resent message is not double-prefixed.
 */

/**
 * Strip a previously prepended force-search block for display / re-edit.
 */
export function stripForceWebSearchPrefix(text: string): string {
  const t = text.replace(/\r\n/g, "\n");
  if (!t.startsWith("【联网检索】")) return text;
  const sep = "\n---\n\n";
  const i = t.indexOf(sep);
  if (i >= 0) return t.slice(i + sep.length);
  // Fallback: drop first line block until blank line
  const m = t.match(/^【联网检索】[\s\S]*?\n\n---\n\n?([\s\S]*)$/);
  return m ? m[1] : text;
}

export function detectForceWebSearchInText(text: string): boolean {
  return text.replace(/\r\n/g, "\n").startsWith("【联网检索】");
}
