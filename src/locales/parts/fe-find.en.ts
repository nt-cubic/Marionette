import type { zh } from "./fe-find.zh";

/** English mirror of the fe-find fragment. */
export const en = {
  "find.placeholder": "Find in conversation",
  "find.counter": "{i} / {n}",
  "find.noMatch": "No matches",
  "find.hint": "Searches loaded conversation cards: you / replies / plans / questions / handoffs / subagents / changes",
  "find.prev": "Previous match (Shift Enter)",
  "find.next": "Next match (Enter)",
  "find.close": "Close find",
  "find.paletteOpen": "Find in current conversation",
} satisfies Record<keyof typeof zh, string>;
