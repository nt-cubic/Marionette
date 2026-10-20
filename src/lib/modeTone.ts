/**
 * Soft visual family for the mode rail / chip / reply meta.
 * Kept muted (not rainbow); DSH agent presets get distinct tones so
 * Standard / PTC / Minimal / Creator are not all gray.
 */
export function modeTone(modeId: string | null | undefined): string {
  const id = (modeId ?? "").trim().toLowerCase();
  if (!id) return "default";

  // DeepSeek Harness agent presets (configOption id `agent`).
  if (id === "standard") return "build";
  if (id === "ptc") return "ask";
  if (id === "minimal") return "minimal";
  if (id === "cordis" || id === "creator" || id === "creation") return "debug";

  if (id.includes("plan")) return "plan";
  if (id.includes("ask") || id.includes("chat") || id.includes("talk")) return "ask";
  if (id.includes("debug") || id.includes("review")) return "debug";
  // Full access / unrestricted: distinct from normal agent (workspace sandbox).
  if (
    id.includes("full-access") ||
    id.includes("full_access") ||
    id.includes("bypass") ||
    id.includes("yolo")
  ) {
    return "build";
  }
  if (
    id.includes("build") ||
    id.includes("agent") ||
    id.includes("code") ||
    id.includes("edit") ||
    id.includes("default") ||
    id.includes("auto")
  ) {
    return "build";
  }
  return "default";
}
