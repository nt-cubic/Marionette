import type { zh } from "./fe-seenrail.zh";

/** English mirror of the fe-seenrail fragment. */
export const en = {
  "seenImages.rail.expand": "Expand images",
  "seenImages.rail.collapse": "Collapse images",
  "seenImages.rail.moreTile": "{n} more — expand",
  "seenImages.rail.more": "Show more · {shown} / {total}",
  "seenImages.rail.empty": "No images under this filter",
} satisfies Record<keyof typeof zh, string>;
