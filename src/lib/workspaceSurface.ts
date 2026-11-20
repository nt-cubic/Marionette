/** Stable document tab id plus the one browser display policy used by App. */
export const DOCUMENT_SURFACE_PREFIX = "document:";

export function documentSurfaceTabId(path: string): string {
  return DOCUMENT_SURFACE_PREFIX + path;
}

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
