import type { zh } from "./fe-seenshell.zh";

/** English mirror of the fe-seenshell fragment. */
export const en = {
  "seenImages.title": "Images",
  "seenImages.open": "View {name}",
  "seenImages.via.you": "Attached by you",
  "seenImages.via.browser": "Browser screenshot",
  "seenImages.via.computer": "Desktop screenshot",
  "seenImages.via.read": "Read image",
  "seenImages.group.now": "Just now",
  "seenImages.group.today": "Today",
  "seenImages.group.yesterday": "Yesterday",
  "seenImages.group.day": "{month}/{day}",
  "seenImages.filter.all": "All {total}",
  "seenImages.filter.you": "Yours {total}",
  "seenImages.filter.tool": "Tools {total}",
  "seenImages.meta": "{total} · ≈ {tokens} tokens",
  "seenImages.partial": "Loaded part only · older images appear as you load earlier messages",
  "seenImages.tip": "{via} · {time}",
  "seenImages.notSentModel": "The catalog lists the current model {model} as text-only: these images were not delivered to it.",
  "seenImages.notSentAgent": "{agent} does not accept images: these images were not delivered to it.",
  "seenImages.switchModel": "Switch to a model that can see images",
  "seenImages.locateMissing": "Could not find the card for this image: earlier messages are not loaded yet. Load earlier messages first.",
} satisfies Record<keyof typeof zh, string>;
