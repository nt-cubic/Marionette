import type { zh } from "./fe-seenview.zh";

/** English mirror of the fe-seenview fragment. */
export const en = {
  "seenImages.viewer.aria": "Image viewer",
  "seenImages.viewer.prev": "Previous image",
  "seenImages.viewer.next": "Next image",
  "seenImages.viewer.locate": "Locate in conversation",
  "seenImages.viewer.reveal": "Reveal in folder",
  "seenImages.viewer.copyPath": "Copy path",
  "seenImages.viewer.copied": "Copied",
  "seenImages.viewer.keys": "← → page · L locate · Esc close",
  "seenImages.viewer.thumb": "Image {n}",
} satisfies Record<keyof typeof zh, string>;
