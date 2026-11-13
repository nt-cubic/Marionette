/**
 * Soft visual family for the mode rail / chip / reply meta.
 * Kept muted (not rainbow); DSH agent presets get distinct tones so
 * Standard / PTC / Minimal / Creator are not all gray.
 *
 * 2026-11-13 second rule (user report): within ONE agent, no two modes may
 * share a tone — cross-agent repeats are fine. Driven by real mode lists:
 * claude had five greens (Manual/AcceptEdits/Don'tAsk/Auto/Bypass), zcode
 * three (build/edit/yolo), codex two grays (read-only/workspace-write).
 * The "loose" tone (danger red) was added as the seventh family so the
 * wide-open modes (bypass / full-access / yolo) stop piling onto green.
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
  // ZCode's tightest sandbox — give the restrictive end a voice (ASK blue)
  // instead of toneless gray, mirroring read-only's role on other agents.
  if (id === "guarded") return "ask";
  // ZCode's plain "edit" and claude's bare "Auto" would both collide with
  // their list-mates on the build green / via the generic auto rule — move
  // each onto a free tone of its own list (teal / orange).
  if (id === "edit") return "minimal";
  if (id === "auto") return "debug";

  if (id.includes("plan")) return "plan";
  // Claude's Don't Ask stays in the asking family (blue): its literal subject
  // is asking behavior, and the build slot belongs to Manual/Auto in the same
  // list — within-agent distinctness outranks the autonomy reading here.
  if (id.includes("ask") || id.includes("chat") || id.includes("talk")) return "ask";
  if (id.includes("debug") || id.includes("review")) return "debug";
  // Restrictive readers (codex Read-only, …) — the asking family's blue beats
  // toneless gray now that the same list also carries workspace-write.
  if (id.includes("read-only") || id.includes("read_only") || id === "read") return "ask";
  // Accept Edits — a quiet, limited autonomy; teal keeps it apart from
  // Manual's green and Don't Ask's blue in the same claude list.
  if (id.includes("accept-edits") || id.includes("acceptedits")) return "minimal";
  // Wide open: bypass / full access / yolo — danger red, NOT the build green
  // they used to share with every agent's working mode.
  if (
    id.includes("full-access") ||
    id.includes("full_access") ||
    id.includes("bypass") ||
    id.includes("yolo")
  ) {
    return "loose";
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
