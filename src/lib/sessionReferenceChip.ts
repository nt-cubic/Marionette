/**
 * Composer session-reference chips (P1-4, first version).
 *
 * One explicit reference = one explicitly loaded transcript tail + the bounded
 * snapshot from `buildSessionReference`. Nothing is read until the user pastes
 * a Marionette session link, drops a session row, or publishes the DOM event
 * below. No permissions are changed: the only read is the existing
 * `load_transcript` command, and the snapshot builder redacts credentials.
 */
import {
  CHAT_PROJECT_ID,
  isTauriRuntime,
  listChatSessions,
  listProjects,
  listSessions,
  loadTranscriptPageStrict,
} from "./api";
import { findLinkTargets } from "./linkTargets";
import { buildSessionReference, type SessionReferenceSnapshot } from "./sessionReference";
import { parseTranscriptEvents } from "./transcript";
import type { Session } from "./types";

// --- Public contract -------------------------------------------------------

/** Custom MIME for an in-page session-row drag (HTML5 only, like memo drags). */
export const SESSION_REFERENCE_DRAG_TYPE = "application/x-marionette-session-reference";

/** Window event for future sidebar/inspector "add to composer" buttons. */
export const SESSION_REFERENCE_EVENT = "marionette:add-session-reference";

/** Hard cap of chips per composer; keeps the send payload bounded in number too. */
export const SESSION_REFERENCE_MAX_CHIPS = 8;

/**
 * Only the tail of the source transcript is loaded. `buildSessionReference`
 * scans at most 1500 events by default, so this window is enough for its
 * default scan budget while never materializing a 24 MB transcript.
 */
export const SESSION_REFERENCE_TAIL_EVENTS = 2000;

export type SessionReferenceTarget = {
  sessionId: string;
  projectId?: string | null;
  agentId?: string | null;
  label?: string | null;
};

export type SessionReferenceChipError =
  | "desktop_required"
  | "not_found"
  | "read_failed";

export type SessionReferenceChipState = {
  target: SessionReferenceTarget;
  status: "loading" | "ready" | "error";
  snapshot: SessionReferenceSnapshot | null;
  error: SessionReferenceChipError | null;
  errorDetail: string | null;
};

// --- Link / drag / event parsing -------------------------------------------

const SESSION_ID_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,199}$/;

function cleanId(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return SESSION_ID_RE.test(trimmed) ? trimmed : null;
}

function cleanOptional(value: unknown, max = 200): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed ? trimmed.slice(0, max) : null;
}

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

/** Validate/normalize a paste, drop or event payload. */
export function normalizeSessionReferenceTarget(input: unknown): SessionReferenceTarget | null {
  if (!input || typeof input !== "object") return null;
  const record = input as Record<string, unknown>;
  const sessionId = cleanId(record.sessionId ?? record.id);
  if (!sessionId) return null;
  return {
    sessionId,
    projectId: cleanId(record.projectId),
    agentId: cleanOptional(record.agentId, 120),
    label: cleanOptional(record.label, 160),
  };
}

const SESSION_LINK_RE =
  /^marionette:(?:\/\/)?(?:session|sessions)\/([^?#\s]+)(?:\?([^\s#]*))?$/i;

/**
 * `marionette://session/<id>` (project/agent/label optional query) is the link
 * form; a bare link-only paste becomes a chip instead of draft text.
 */
export function parseSessionReferenceText(text: string): SessionReferenceTarget | null {
  const trimmed = text.trim();
  if (!trimmed || trimmed.length > 2048) return null;
  const match = SESSION_LINK_RE.exec(trimmed);
  if (!match) return null;
  const sessionId = cleanId(safeDecode(match[1] ?? ""));
  if (!sessionId) return null;
  let projectId: string | null = null;
  let agentId: string | null = null;
  let label: string | null = null;
  if (match[2]) {
    const query = new URLSearchParams(match[2]);
    projectId = cleanId(query.get("project") ?? query.get("projectId"));
    agentId = cleanOptional(query.get("agent") ?? query.get("agentId"), 120);
    label = cleanOptional(query.get("label"), 160);
  }
  return { sessionId, projectId, agentId, label };
}

/** Canonical link for copy/UI and the text/plain half of a session-row drag. */
export function sessionReferenceLink(target: SessionReferenceTarget): string {
  const query = new URLSearchParams();
  if (target.projectId) query.set("project", target.projectId);
  if (target.agentId) query.set("agent", target.agentId);
  if (target.label) query.set("label", target.label);
  const suffix = query.toString();
  return `marionette://session/${encodeURIComponent(target.sessionId)}${suffix ? `?${suffix}` : ""}`;
}

/** Future sidebar row: stamp both the custom type and a plain-link fallback. */
export function writeSessionReferenceDrag(
  dataTransfer: DataTransfer,
  target: SessionReferenceTarget,
): void {
  dataTransfer.setData(SESSION_REFERENCE_DRAG_TYPE, JSON.stringify(target));
  dataTransfer.setData("text/plain", sessionReferenceLink(target));
}

export function hasSessionReferenceDrag(dataTransfer: DataTransfer): boolean {
  return Array.from(dataTransfer.types ?? []).includes(SESSION_REFERENCE_DRAG_TYPE);
}

/** Read a session-row drop; falls back to a pasted link in text/plain. */
export function readSessionReferenceDrag(dataTransfer: DataTransfer): SessionReferenceTarget | null {
  const raw = dataTransfer.getData(SESSION_REFERENCE_DRAG_TYPE);
  if (raw) {
    try {
      const parsed = normalizeSessionReferenceTarget(JSON.parse(raw));
      if (parsed) return parsed;
    } catch {
      // fall through to the plain link
    }
  }
  return parseSessionReferenceText(dataTransfer.getData("text/plain"));
}

/** DOM event path for a future sidebar button / context-menu action. */
export function publishSessionReferenceToComposer(target: SessionReferenceTarget): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(
    new CustomEvent<SessionReferenceTarget>(SESSION_REFERENCE_EVENT, { detail: target }),
  );
}

// --- Per-dialog chip cache (mirrors the draft cache lifecycle) -------------

const chipsByComposerSession = new Map<string, SessionReferenceChipState[]>();

export function readSessionReferenceCache(sessionId: string): SessionReferenceChipState[] {
  if (!sessionId) return [];
  const cached = chipsByComposerSession.get(sessionId) ?? [];
  // A remount must not resurrect an in-flight read; it is not sendable and
  // would stay "loading" forever because its promise belonged to the old tree.
  return cached.filter((chip) => chip.status !== "loading");
}

export function writeSessionReferenceCache(
  sessionId: string,
  chips: SessionReferenceChipState[],
): void {
  if (!sessionId) return;
  if (chips.length > 0) chipsByComposerSession.set(sessionId, chips);
  else chipsByComposerSession.delete(sessionId);
}

export function clearSessionReferenceCache(sessionId: string): void {
  if (!sessionId) return;
  chipsByComposerSession.delete(sessionId);
}

// --- Snapshot loading ------------------------------------------------------

function errorChip(
  target: SessionReferenceTarget,
  error: SessionReferenceChipError,
  detail: string | null,
): SessionReferenceChipState {
  return { target, status: "error", snapshot: null, error, errorDetail: detail };
}

async function findSession(target: SessionReferenceTarget): Promise<Session | null> {
  const matches = (session: Session) => session.id === target.sessionId;

  if (target.projectId) {
    const list =
      target.projectId === CHAT_PROJECT_ID
        ? await listChatSessions()
        : await listSessions(target.projectId);
    const hit = list.find(matches);
    if (hit) return hit;
  }

  if (target.projectId !== CHAT_PROJECT_ID) {
    const chats = await listChatSessions();
    const chatHit = chats.find(matches);
    if (chatHit) return chatHit;
  }

  // Link-only paste: the session may live in any project. This only lists
  // metadata (never transcript bodies) and runs solely on an explicit action.
  const projects = await listProjects();
  for (const project of projects) {
    const list = await listSessions(project.id);
    const hit = list.find(matches);
    if (hit) return hit;
  }
  return null;
}

function errorDetail(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error);
  return raw.length > 240 ? `${raw.slice(0, 240)}...` : raw;
}

/**
 * Explicitly read one session: metadata lookup + transcript tail + bounded
 * snapshot. The transcript never leaves this function; only the snapshot is
 * kept by the chip.
 */
export async function loadSessionReferenceChip(
  target: SessionReferenceTarget,
): Promise<SessionReferenceChipState> {
  if (!isTauriRuntime()) return errorChip(target, "desktop_required", null);

  try {
    const session = await findSession(target);
    if (!session) return errorChip(target, "not_found", null);

    const page = await loadTranscriptPageStrict(session.id, SESSION_REFERENCE_TAIL_EVENTS);
    const events = parseTranscriptEvents(page.events);
    const snapshot = buildSessionReference({
      session,
      events,
      sourceHeadTruncated: page.firstLine > 0,
    });
    return { target, status: "ready", snapshot, error: null, errorDetail: null };
  } catch (err) {
    return errorChip(target, "read_failed", errorDetail(err));
  }
}

const ZERO_WIDTH = "\u200b";

/**
 * The send pipeline scans composed text for path-like tokens and asks for a
 * workspace grant when one resolves outside the project. Reference paths are
 * quoted history, not a request to read those files, so split every detected
 * path separator with a zero-width space: the model still reads the same text,
 * while the grant scanner (same `findLinkTargets`) no longer treats it as a
 * live path. This is the no-permission-widening guarantee for P1-4.
 */
export function guardSessionReferencePaths(text: string): string {
  const paths = findLinkTargets(text)
    .filter((target) => target.kind === "path")
    .sort((a, b) => b.start - a.start);
  let out = text;
  for (const target of paths) {
    const raw = out.slice(target.start, target.end);
    if (!raw) continue;
    const guarded = raw.replace(/([\\/])/g, `$1${ZERO_WIDTH}`);
    out = `${out.slice(0, target.start)}${guarded}${out.slice(target.end)}`;
  }
  return out;
}

/** Append the bounded snapshots after the user's own words, in chip order. */
export function withSessionReferenceText(
  text: string,
  chips: readonly SessionReferenceChipState[],
): string {
  const blocks = chips
    .filter((chip) => chip.status === "ready" && chip.snapshot?.text)
    .map((chip) => guardSessionReferencePaths(chip.snapshot!.text));
  if (blocks.length === 0) return text;
  return [text.trim(), ...blocks].filter(Boolean).join("\n\n");
}
