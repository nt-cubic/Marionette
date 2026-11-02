import type { zh } from "./fe-update.zh";

/** English mirror of the fe-update fragment. */
export const en = {
  "appUpdate.downloading": "Downloading update {version}…",
  "appUpdate.failed": "Update failed: {error}",
  "appUpdate.failedHint": "Common cause: the connection dropped. Enable a proxy in settings and retry.",
  "appUpdate.applying": "Replacing the program and restarting…",
  "appUpdate.noPending": "The update package has not finished downloading; retry the download.",
} satisfies Record<keyof typeof zh, string>;
