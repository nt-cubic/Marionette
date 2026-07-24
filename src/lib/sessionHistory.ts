import type { SessionEvent } from "./types";

const MAX_CHARS = 28_000;
const MAX_TURNS = 40; // user+assistant pairs roughly
const MAX_MSG_CHARS = 2_400;

function clip(text: string, max = MAX_MSG_CHARS): string {
  const t = text.trim();
  if (t.length <= max) return t;
  return `${t.slice(0, max - 1)}…`;
}

/**
 * Build a prompt prefix that rehydrates agent context from local Clean transcript.
 *
 * Why: AgentShell persists UI history in JSONL, but ACP always does session/new
 * after restart — the agent process has empty memory. Until session/load is
 * reliable across OpenCode/Claude/etc., we inject prior turns on the first
 * prompt of each fresh ACP connection.
 *
 * Does NOT include the current user message (caller appends that).
 */
export function buildHistoryInjection(
  events: SessionEvent[],
  sessionId: string,
): string | null {
  const prior = events.filter((e) => e.sessionId === sessionId);
  const lines: string[] = [];
  let turns = 0;

  for (const e of prior) {
    if (e.type === "user_message") {
      lines.push(`User:\n${clip(e.text)}`);
      turns += 1;
    } else if (e.type === "assistant_message") {
      // Skip shell error cards
      if (e.text.startsWith("**") && /error|Agent error|Sign in/i.test(e.text.slice(0, 80))) {
        continue;
      }
      lines.push(`Assistant:\n${clip(e.text)}`);
    } else if (e.type === "tool_call") {
      const title = e.title || e.text.split("\n")[0] || "tool";
      lines.push(`Tool: ${clip(title, 200)}${e.status ? ` (${e.status})` : ""}`);
    }
    // Skip thought / handoff noise for context budget
    if (turns >= MAX_TURNS) break;
  }

  if (lines.length === 0) return null;

  let body = lines.join("\n\n");
  if (body.length > MAX_CHARS) {
    body = `…(earlier turns omitted)\n\n${body.slice(body.length - MAX_CHARS)}`;
  }

  return [
    "[AgentShell — prior conversation in this dialog]",
    "The UI has history from earlier in this thread. Your ACP session was freshly started",
    "(e.g. app restart), so you do NOT have this context yet. Treat the following as",
    "what already happened. Continue naturally; do not claim the history is empty.",
    "",
    body,
    "",
    "[End of prior conversation. The user's new message follows.]",
    "",
  ].join("\n");
}

/** Prefix user text with history injection when non-null. */
export function withHistoryInjection(
  historyPrefix: string | null,
  userText: string,
): string {
  if (!historyPrefix) return userText;
  return `${historyPrefix}${userText}`;
}
