/**
 * Event contract between the orb window (label "orb") and the main window.
 *
 * The orb is a remote control: it renders UI and emits OrbCommand events;
 * the main window (hidden but alive in background mode) executes them
 * through the same pipelines the Telegram bridge uses. State flows back
 * as OrbState events. All events are broadcast via tauri `emit`; handlers
 * filter by the `origin` window label (same pattern as detachedWindow.ts).
 */

import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauriRuntime } from "./api";

export const ORB_WINDOW_LABEL = "orb";
/** Main window -> orb: running tasks, pending notices, main-window visibility. */
export const ORB_STATE_EVENT = "marionette-orb-state";
/** Orb -> main window: ask the executor to do something. */
export const ORB_COMMAND_EVENT = "marionette-orb-command";

export type OrbNoticeKind = "reply" | "permission" | "error";

export type OrbNotice = {
  kind: OrbNoticeKind;
  sessionId: string;
  /** Session display label at notice time. */
  label: string;
  /** Epoch ms — the orb dedupes/acks by this timestamp. */
  at: number;
};

export type OrbRunningTask = {
  sessionId: string;
  label: string;
  agentLabel: string;
  /** Epoch ms when the current turn started. */
  startedAt: number;
};

export type OrbState = {
  /** False while the main window is hidden to background. */
  mainVisible: boolean;
  running: OrbRunningTask[];
  /** Unacknowledged notices; the orb clears them via `clearNotices`. */
  notices: OrbNotice[];
};

/** Normalized capture content dragged/pasted/typed into the orb. */
export type OrbCapturePayload = {
  /** Plain text (typed memo, dragged selection, link list fallback). */
  text: string;
  /** URLs found in the drop (text/uri-list) — already enriched (title/image
   * downloaded) by the orb before sending. */
  urls: string[];
  /** Raw text/html fragment, when the source provided one. */
  html?: string;
  /** Absolute local file paths (blobs saved / images downloaded by Rust). */
  filePaths: string[];
};

export type OrbCommand =
  | { kind: "newChat"; agentId?: string }
  | { kind: "openSession"; sessionId: string }
  | { kind: "showMain" }
  | { kind: "clearNotices" }
  | { kind: "scenario"; scenarioId: string; payload?: OrbCapturePayload }
  | { kind: "capture"; payload: OrbCapturePayload };

function currentLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return "";
  }
}

/** Orb side: ask the main window to execute a command (fire-and-forget). */
export async function emitOrbCommand(command: OrbCommand): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await emit(ORB_COMMAND_EVENT, { ...command, origin: currentLabel() });
  } catch {
    /* best-effort */
  }
}

/** Main side: broadcast state to the orb. */
export async function emitOrbState(state: OrbState): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await emit(ORB_STATE_EVENT, { ...state, origin: currentLabel() });
  } catch {
    /* best-effort */
  }
}

/** Main side: receive orb commands (ignores own broadcasts). */
export async function listenOrbCommands(
  handler: (command: OrbCommand) => void
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => undefined;
  const own = currentLabel();
  return listen<OrbCommand & { origin?: string }>(ORB_COMMAND_EVENT, (event) => {
    const { origin, ...command } = event.payload;
    if (origin && origin === own) return;
    if (!command || typeof command.kind !== "string") return;
    void handler(command as OrbCommand);
  });
}

/** Orb side: receive state updates (ignores own broadcasts). */
export async function listenOrbState(
  handler: (state: OrbState) => void
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => undefined;
  const own = currentLabel();
  return listen<OrbState & { origin?: string }>(ORB_STATE_EVENT, (event) => {
    const { origin, ...state } = event.payload;
    if (origin && origin === own) return;
    if (!state || typeof state.mainVisible !== "boolean") return;
    void handler(state as OrbState);
  });
}
