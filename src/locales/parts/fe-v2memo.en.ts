import type { zh } from "./fe-v2memo.zh";

export const en = {
  "v2memo.compose": "Write a note or paste a link…",
  "v2memo.composeHint": "Ctrl+Enter to save · drop files or text anywhere in the window",
  "v2memo.selected": "{n} selected",
  "v2memo.selHint": "Prefills the composer, nothing is sent automatically",
  "v2memo.cancelSelect": "Clear selection",
} satisfies Record<keyof typeof zh, string>;
