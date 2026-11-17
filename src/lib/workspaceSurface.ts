/**
 * Small, DOM-free rules for the conversation + surface workspace.
 * Browser is the first registered surface; session/document/memo names stay
 * in the union so later migrations do not need a second layout vocabulary.
 */

export type WorkspaceSurfaceKind = "browser" | "session" | "document" | "memo";

export type WorkspaceSurfaceRef =
  | { kind: "browser"; id: string | null }
  | { kind: "session"; id: string }
  | { kind: "document"; id: string }
  | { kind: "memo"; id: null };

export const WORKSPACE_SURFACE_MIN_PX = 360;
export const WORKSPACE_CHAT_MIN_PX = 320;
export const WORKSPACE_SURFACE_STACK_PX = 760;
export const WORKSPACE_SURFACE_DEFAULT_RATIO = 0.62;

export function browserTabToOpenInWorkspace(
  fresh: readonly { id: string; display: "background" | "workspace" }[],
  suppressActivate = false,
): string | null {
  if (suppressActivate) return null;
  for (let index = fresh.length - 1; index >= 0; index -= 1) {
    const tab = fresh[index]!;
    if (tab.display === "workspace") return tab.id;
  }
  return null;
}

export function shouldStackWorkspace(totalWidth: number): boolean {
  return totalWidth > 0 && totalWidth < WORKSPACE_SURFACE_STACK_PX;
}

export function clampWorkspaceSurfaceWidth(
  requested: number,
  totalWidth: number,
  minSurface = WORKSPACE_SURFACE_MIN_PX,
  minChat = WORKSPACE_CHAT_MIN_PX,
): number {
  if (
    !Number.isFinite(requested) ||
    !Number.isFinite(totalWidth) ||
    totalWidth <= 0
  ) {
    return minSurface;
  }
  const maxSurface = Math.max(minSurface, totalWidth - minChat);
  return Math.round(Math.min(Math.max(requested, minSurface), maxSurface));
}

/** Width implied by the pointer for a surface on either side of the seam. */
export function workspaceSurfaceWidthFromPointer(
  clientX: number,
  rect: { left: number; right: number; width: number },
  side: "left" | "right",
): number {
  const requested =
    side === "left" ? clientX - rect.left : rect.right - clientX;
  return clampWorkspaceSurfaceWidth(requested, rect.width);
}
