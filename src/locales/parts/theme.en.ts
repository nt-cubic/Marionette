import type { zh } from "./theme.zh";

/** English mirror of the theme fragment. */
export const en = {
  "theme.dim": "Dusk",
  "theme.white": "Pure White",
  "theme.toggleTitle": "Theme: {current} (click to switch to {next})",
} satisfies Record<keyof typeof zh, string>;
