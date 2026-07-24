/** A pinned inline comment on selected Clean text (not yet sent). */
export type QuotePin = {
  id: string;
  quoted: string;
  comment: string;
  /** Position relative to the event-list scroll container. */
  x: number;
  y: number;
};

/**
 * Build the outbound prompt from pins + optional free Composer text.
 *
 * 1.
 * > quoted
 * 评论：…
 *
 * 2.
 * …
 *
 * (composer free text last, if any)
 */
export function formatPinsForSend(pins: QuotePin[], freeText: string): string {
  const blocks = pins.map((pin, index) => {
    const quoted = pin.quoted.replace(/\r\n/g, "\n").trim();
    const lines = quoted.split("\n").map((line) => `> ${line}`);
    const comment = pin.comment.replace(/\r\n/g, "\n").trim();
    return `${index + 1}.\n${lines.join("\n")}\n评论：${comment}`;
  });
  const free = freeText.replace(/\r\n/g, "\n").trim();
  if (free) blocks.push(free);
  return blocks.join("\n\n");
}

export function newQuotePinId(): string {
  return `qp-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}
