import type { SessionEvent } from "./types";
import { ansiToPlainText } from "./ansi";

/**
 * PTY → Clean transcript bridge.
 *
 * Clean View must show You / Thinking / Tool / Reply for every agent.
 * We never dump raw TUI chrome; we only promote classified, de-duplicated lines
 * after the user has sent a Composer message (armed turn).
 */

export type PtyBridgeState = {
  lineBuf: string;
  armed: boolean;
  lastUserText: string | null;
  /** Per-turn fingerprints to kill TUI redraw duplicates. */
  seen: Set<string>;
};

export function createPtyBridgeState(): PtyBridgeState {
  return { lineBuf: "", armed: false, lastUserText: null, seen: new Set() };
}

export function armPtyBridge(state: PtyBridgeState, userText: string): PtyBridgeState {
  return {
    lineBuf: "",
    armed: true,
    lastUserText: userText,
    seen: new Set(),
  };
}

function isMostlyBoxDrawing(line: string): boolean {
  const t = line.trim();
  if (!t) return true;
  const chrome = t.replace(
    /[^\u2500-\u257F\u2580-\u259F\u25A0-\u25FF│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▀■▪▸▾◆◇●○]+/g,
    "",
  );
  return chrome.length >= Math.max(2, t.length * 0.35);
}

function isChromeLine(line: string): boolean {
  const t = line.trim();
  if (!t) return true;
  if (isMostlyBoxDrawing(t)) return true;

  // Grok / agent TUI chrome
  if (/Shift\+Tab|Ctrl\+|always-approve/i.test(t)) return true;
  if (/\d+K\s*\/\s*\d+K/.test(t)) return true;
  if (/Grok Build Beta|is here!|New worktree|Resume session|Changelog|^\s*Quit\s*$/i.test(t)) return true;
  if (/Select ['"]Grok|under \/model/i.test(t)) return true;
  if (/Turn completed in\s*[\d.]+s/i.test(t)) return true;
  if (/^\d{1,2}:\d{2}\s*(AM|PM)\s*$/i.test(t)) return true;
  if (/\d{1,2}:\d{2}\s*(AM|PM)/i.test(t) && t.length < 24) return true;
  if (/^master\s+/i.test(t) && /AgentsShell|\\|\//.test(t)) return true;
  if (/^[|│\s]{3,}$/.test(t)) return true;
  // Prompt-only leftovers
  if (/^[>|❯]\s*$/.test(t)) return true;
  if (/^[>|❯]\s*[>|❯]\s*$/.test(t)) return true;
  return false;
}

function normalizeForCompare(s: string): string {
  return s
    .toLowerCase()
    .replace(/^[>|❯◆●\s]+/, "")
    .replace(/\s+/g, " ")
    .trim();
}

function isUserEcho(line: string, lastUser: string | null): boolean {
  if (!lastUser) return false;
  const a = normalizeForCompare(line);
  const b = normalizeForCompare(lastUser);
  if (!a || !b) return false;
  if (a === b) return true;
  // TUI often re-prints "> user text"
  if (a === `> ${b}` || a.endsWith(b) && a.length <= b.length + 4) return true;
  return false;
}

/** Strip chrome fragments glued onto an otherwise useful line. */
function scrubContent(line: string): string {
  let t = line.trim();
  t = t.replace(/^[>|❯]\s+/, "");
  t = t.replace(/\s*Turn completed in\s*[\d.]+s\.?/gi, "");
  t = t.replace(/\s*\d{1,2}:\d{2}\s*(AM|PM)/gi, "");
  t = t.replace(/\s*[>|❯]\s*$/g, "");
  t = t.replace(/\s*\|\s*$/g, "");
  t = t.replace(/\s{2,}/g, " ").trim();
  return t;
}

type Kind = "thought" | "tool" | "reply" | "skip";

function classifyLine(line: string): Kind {
  const t = line.trim();
  if (!t) return "skip";
  if (/thought for\s+[\d.]+s/i.test(t) || /^◆\s*Thought\b/i.test(t) || /^thinking\b/i.test(t)) {
    return "thought";
  }
  if (
    /^◆\s*(List|Run|Read|Edit|Bash|Search|Grep|Write|Glob|Interjected|Tool)\b/i.test(t) ||
    /^●\s*(List|Run|Read|Edit|Bash)\b/i.test(t) ||
    /^Interjected:/i.test(t)
  ) {
    return "tool";
  }
  if (/^[>|❯]\s*$/.test(t)) return "skip";
  return "reply";
}

function fingerprint(kind: string, text: string): string {
  return `${kind}:${normalizeForCompare(text).slice(0, 240)}`;
}

function appendEvent(
  events: SessionEvent[],
  sessionId: string,
  kind: "thought" | "tool" | "assistant_message",
  text: string,
): SessionEvent[] {
  const last = events[events.length - 1];
  if (last && last.sessionId === sessionId && last.type === kind && "text" in last) {
    const prev = last.text;
    const nPrev = normalizeForCompare(prev);
    const nText = normalizeForCompare(text);
    if (!nText) return events;
    // Exact / suffix duplicate (redraw)
    if (nPrev === nText || nPrev.endsWith(nText) || prev.includes(text)) {
      return events;
    }
    // Thought headers: keep a single card, prefer the first marker
    if (kind === "thought" && /thought for/i.test(nPrev) && /thought for/i.test(nText)) {
      return events;
    }
    const next = [...events];
    next[next.length - 1] = {
      ...last,
      text: prev.endsWith("\n") || prev.length === 0 ? prev + text : `${prev}\n${text}`,
    };
    return next;
  }

  const createdAt = new Date().toISOString();
  if (kind === "thought") {
    return [...events, { type: "thought", sessionId, text, createdAt }];
  }
  if (kind === "tool") {
    const title = text.replace(/^[◆●]\s*/, "").split(/\s+/).slice(0, 4).join(" ");
    return [...events, { type: "tool_call", sessionId, text, title, status: "running", createdAt }];
  }
  return [...events, { type: "assistant_message", sessionId, text, createdAt }];
}

/**
 * Ingest a raw PTY chunk. Applies at most one React state update via mapEvents.
 */
export function ingestPtyOutput(
  state: PtyBridgeState,
  sessionId: string,
  rawChunk: string,
  mapEvents: (fn: (prev: SessionEvent[]) => SessionEvent[]) => void,
): PtyBridgeState {
  if (!state.armed) {
    return state;
  }

  const plain = ansiToPlainText(rawChunk);
  let lineBuf = state.lineBuf + plain;
  // Also split on bare CR (some TUIs use CR without LF for line updates)
  lineBuf = lineBuf.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const parts = lineBuf.split("\n");
  lineBuf = parts.pop() ?? "";

  const seen = new Set(state.seen);
  const ops: Array<(events: SessionEvent[]) => SessionEvent[]> = [];

  for (const rawLine of parts) {
    let line = rawLine.replace(/\s+$/g, "");
    if (isChromeLine(line)) continue;
    if (isUserEcho(line, state.lastUserText)) continue;

    const kind = classifyLine(line);
    if (kind === "skip") continue;

    let text = scrubContent(line);
    if (!text) continue;
    if (isChromeLine(text)) continue;
    if (isUserEcho(text, state.lastUserText)) continue;

    // Drop residual prompt prefix on replies
    if (kind === "reply") {
      text = text.replace(/^[>|❯]\s+/, "").trim();
      if (!text || isUserEcho(text, state.lastUserText)) continue;
      // Ignore ultra-short junk after scrub ("|", ">", etc.)
      if (text.length <= 1) continue;
    }

    const fp = fingerprint(kind, text);
    if (seen.has(fp)) continue;
    seen.add(fp);

    if (kind === "thought") {
      ops.push((prev) => appendEvent(prev, sessionId, "thought", text));
    } else if (kind === "tool") {
      ops.push((prev) => appendEvent(prev, sessionId, "tool", text));
    } else {
      ops.push((prev) => appendEvent(prev, sessionId, "assistant_message", text));
    }
  }

  if (ops.length > 0) {
    mapEvents((prev) => {
      let e = prev;
      for (const op of ops) e = op(e);
      return e;
    });
  }

  return { ...state, lineBuf, seen };
}

/** Flush incomplete line buffer (e.g. debounce after idle). */
export function flushPtyBridge(
  state: PtyBridgeState,
  sessionId: string,
  mapEvents: (fn: (prev: SessionEvent[]) => SessionEvent[]) => void,
): PtyBridgeState {
  if (!state.armed || !state.lineBuf.trim()) {
    return { ...state, lineBuf: "" };
  }
  // Treat remainder as a synthetic line
  return ingestPtyOutput({ ...state, lineBuf: "" }, sessionId, state.lineBuf + "\n", mapEvents);
}
