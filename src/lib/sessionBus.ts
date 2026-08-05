/**
 * Cross-window session metadata bus.
 *
 * Detached dialog windows and the main shell share ACP in Rust, but each
 * webview has its own React state. Broadcast lightweight label/status patches
 * so titles and busy dots stay roughly in sync.
 */

import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauriRuntime } from "./api";
import type { SessionStatus } from "./types";

export const SESSION_PATCH_EVENT = "marionette-session-patch";

export type SessionPatch = {
  sessionId: string;
  label?: string;
  status?: SessionStatus;
  /** Opaque origin window label — ignore echoes from ourselves. */
  origin?: string;
};

let localOrigin: string | null = null;

export async function sessionBusOrigin(): Promise<string> {
  if (localOrigin) return localOrigin;
  if (!isTauriRuntime()) {
    localOrigin = "browser";
    return localOrigin;
  }
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    localOrigin = getCurrentWindow().label || "main";
  } catch {
    localOrigin = "unknown";
  }
  return localOrigin;
}

export async function broadcastSessionPatch(
  patch: Omit<SessionPatch, "origin">
): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const origin = await sessionBusOrigin();
    await emit(SESSION_PATCH_EVENT, { ...patch, origin } satisfies SessionPatch);
  } catch {
    /* optional */
  }
}

export async function listenSessionPatches(
  onPatch: (patch: SessionPatch) => void
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => undefined;
  const origin = await sessionBusOrigin();
  return listen<SessionPatch>(SESSION_PATCH_EVENT, (event) => {
    const patch = event.payload;
    if (!patch?.sessionId) return;
    if (patch.origin && patch.origin === origin) return;
    onPatch(patch);
  });
}
