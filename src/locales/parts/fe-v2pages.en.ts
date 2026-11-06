import type { zh } from "./fe-v2pages.zh";

export const en = {
  "v2pages.specPieChart": "Pie chart",
  "v2pages.diffTab": "Changes",
  "v2pages.diffScope": "Against HEAD",
  "v2pages.diffFiles": "{count} files",
  "v2pages.diffBranch": "branch {branch}",
  "v2pages.diffRefresh": "Refresh changes",
  "v2pages.diffCommit": "Ask AI to commit",
  "v2pages.diffFileList": "Changed files",
  "v2pages.diffNoFiles": "No local changes",
  "v2pages.diffReveal": "Show in folder",
  "v2pages.diffLoading": "Loading diff…",
  "v2pages.diffEmpty": "No diff to show for this file",
} satisfies Record<keyof typeof zh, string>;
