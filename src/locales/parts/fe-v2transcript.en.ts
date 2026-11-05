import type { zh } from "./fe-v2transcript.zh";

export const en = {
  "v2transcript.planApprovalRegion": "Plan approval",
  "v2transcript.previewUnavailable": "Preview unavailable",
  "v2transcript.emptyTitle": "Message {agent}",
  "v2transcript.emptyHint": "Type below to warm the agent in the background, then send when ready.",
  "v2transcript.emptyUpdate": "Update to continue",
  "v2transcript.emptySignIn": "Sign in to continue",
  "v2transcript.emptyUpdateHint": "Use the update button above — it runs `grok update`. Start a new session when it finishes.",
  "v2transcript.emptySignInHint": "Use the Sign in button above — it opens {agent}’s browser login. Come back here when done.",
  "v2transcript.waitingTitle": "Waiting for agent",
  "v2transcript.waitingBody": "Message sent. Streaming will show thinking / tools / reply here.",
} satisfies Record<keyof typeof zh, string>;
