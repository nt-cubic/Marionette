/**
 * Soft visual family for the mode rail / chip / reply meta.
 * Kept muted (not rainbow); DSH agent presets get distinct tones so
 * Standard / PTC / Minimal / Creator are not all gray.
 */
export function modeTone(modeId: string | null | undefined): string {
  // Ids (`full-access`) and display labels (`Full access`) must land on the same tone:
  // the You card and reply footer pass the label, the composer passes the id.
  const id = (modeId ?? "").trim().toLowerCase().replace(/[\s_]+/g, "-");
  if (!id) return "default";

  // DeepSeek Harness agent presets (configOption id `agent`).
  if (id === "standard") return "build";
  if (id === "ptc") return "ask";
  if (id === "minimal") return "minimal";
  if (id === "cordis" || id === "creator" || id === "creation") return "debug";

  // Codex's Auto review: configOption value `agent`, live label "Auto review".
  // The label path lands on debug ("auto-review" includes "review") while the
  // bare id fell through to build — You border / composer rail (id) rendered
  // green against the reply footer's orange (2026-11-13 user report). Pin the
  // id to the label's tone so the whole family (You bar, composer rail, chip,
  // menu swatch, reply footer) agrees on one color per mode.
  if (id === "agent") return "debug";

  // Claude's Don't Ask = stop asking for permissions (autonomous, like
  // Auto/Bypass) — the generic includes("ask") would land it on the ASK
  // family's blue, the exact opposite of what it does.
  if (id === "dontask") return "build";
  // ZCode's tightest sandbox — give the restrictive end a voice (ASK blue)
  // instead of toneless gray, mirroring read-only's role on other agents.
  if (id === "guarded") return "ask";

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
