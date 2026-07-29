import {
  formatAnnotationsForSend,
  quotePinToAnnotation,
} from "./annotations";

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
 * Delegates to `formatAnnotationsForSend` (quote kind) so Annotation and
 * QuotePin stay one formatting path.
 */
export function formatPinsForSend(pins: QuotePin[], freeText: string): string {
  return formatAnnotationsForSend(pins.map(quotePinToAnnotation), freeText);
}

export function newQuotePinId(): string {
  return `qp-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}
