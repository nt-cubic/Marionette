import type { zh } from "./fe-cheaptext.zh";

/** English mirror of the cheap text settings fragment. */
export const en = {
  "cheapText.title": "Auxiliary text model",
  "cheapText.lead":
    "Use one small, inexpensive model for session titles, branch names and commit summaries. It runs outside the conversation and is never counted in agent quota totals; with no usable source, titles keep the first message.",
  "cheapText.enable": "Enable auxiliary text generation",
  "cheapText.source": "Source",
  "cheapText.sourcePlaceholder": "Choose a configured source",
  "cheapText.model": "Model",
  "cheapText.modelPlaceholder": "Enter a model id",
  "cheapText.style": "Title style",
  "cheapText.stylePlaceholder": "Optional, e.g. short and action-oriented",
  "cheapText.maxTokens": "Max output tokens",
  "cheapText.timeout": "Timeout (ms)",
  "cheapText.limitsHint":
    "Output is capped at 16-512 tokens and timeout at 1500-30000 ms. Values outside the range are normalized on save.",
  "cheapText.readinessReady": "Credential ready",
  "cheapText.readinessMissing": "No usable key",
  "cheapText.readinessOff": "Off - first-message titles only",
  "cheapText.readinessUnset": "No source or model selected",
  "cheapText.readinessProtocol": "This source cannot be used for auxiliary text",
  "cheapText.savedReady": "Saved configuration is usable",
  "cheapText.savedMissing": "Saved configuration has no key",
  "cheapText.savedUnset": "No configuration saved yet",
  "cheapText.noSources":
    "No selectable sources yet. Add an API key or a custom source in Model settings first.",
  "cheapText.loading": "Loading settings...",
  "cheapText.save": "Save",
  "cheapText.saving": "Saving...",
  "cheapText.saveFailed": "Could not save",
  "cheapText.savedStatus":
    "Saved. The new configuration applies to later titles, branch names and commit summaries.",
} satisfies Record<keyof typeof zh, string>;
