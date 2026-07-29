import type { ModelDef } from "./types";

/**
 * Display-only model name cleanup. Wire ids stay untouched for set_model / prefs.
 *
 * Structural heuristics (not per-model tables) so ACP label churn does not require
 * a Marionette release each time.
 */

function firstSegment(text: string): string {
  return text
    .split(/[·•●|]/)[0]
    ?.split(/\s+[—–-]\s+\$/)[0]
    ?.split(/\s+\$/)[0]
    ?.trim() ?? text.trim();
}

function stripNoise(text: string): string {
  let s = firstSegment(text);
  // Context / pricing / marketing tails
  s = s.replace(/\s+with\s+\d[\d.]*\s*[kKmM]?\s*context\b.*$/i, "");
  s = s.replace(/\s+\(?\d[\d.]*\s*[kKmM]\s*context\)?\s*$/i, "");
  s = s.replace(/\s+\d[\d.]*k?\s*context\b.*$/i, "");
  s = s.replace(/\s*\$[\d.,]+(?:\s*\/\s*\w+)?\s*$/i, "");
  // Trailing date stamps on ids/labels: -20250514 or -2024-08-06
  s = s.replace(/-\d{8}$/, "");
  s = s.replace(/-\d{4}-\d{2}-\d{2}$/, "");
  return s.trim();
}

function providerPrefix(text: string): { provider: string; rest: string } | null {
  const idx = text.indexOf("/");
  if (idx <= 0 || idx >= text.length - 1) return null;
  return { provider: text.slice(0, idx), rest: text.slice(idx + 1) };
}

/** Humanize common slug tails without maintaining a full catalog. */
function humanizeSlug(slug: string): string {
  let s = slug.trim();
  if (!s) return s;
  // Already spaced / title-ish
  if (/\s/.test(s) && s.length <= 40) return s;

  // grok-4.5 → Grok 4.5
  const grok = s.match(/^grok[-_]?([\d.]+(?:[-_][\w.]+)?)$/i);
  if (grok) {
    const rest = grok[1].replace(/[-_]/g, " ");
    return `Grok ${rest}`;
  }

  // gpt-4o / gpt-5.1-codex-max
  if (/^gpt[-_]/i.test(s) || /^o\d/i.test(s) || /^claude[-_]/i.test(s)) {
    return s
      .replace(/[-_]+/g, " ")
      .replace(/\b([a-z])/g, (c) => c.toUpperCase())
      .replace(/\bGpt\b/g, "GPT")
      .replace(/\bClaude\b/g, "Claude");
  }

  return s;
}

function ellipsize(text: string, max: number): string {
  if (text.length <= max) return text;
  if (max <= 1) return "…";
  return `${text.slice(0, Math.max(1, max - 1)).trimEnd()}…`;
}

/**
 * Short label for menus / chips. Prefer cleaned ACP label; fall back to id tail.
 */
export function prettyModelLabel(
  model: Pick<ModelDef, "id" | "label" | "description">,
  opts?: { maxLen?: number },
): string {
  const maxLen = opts?.maxLen ?? 40;
  const rawLabel = (model.label || "").trim();
  const rawId = (model.id || "").trim();

  let candidate = rawLabel || rawId;
  const fromLabel = providerPrefix(candidate);
  if (fromLabel) candidate = fromLabel.rest;

  candidate = stripNoise(candidate);

  // If label collapsed to something useless, use id tail
  if (!candidate || candidate.length < 2) {
    const idTail = providerPrefix(rawId)?.rest ?? rawId;
    candidate = stripNoise(idTail);
  }

  // Bare family aliases: prefer description head when richer (Claude)
  if (candidate.length <= 12 && model.description) {
    const head = stripNoise(model.description);
    if (head.length > candidate.length) {
      const cl = candidate.toLowerCase();
      const hl = head.toLowerCase();
      if (hl === cl || hl.startsWith(`${cl} `) || hl.includes(cl)) {
        candidate = head;
      }
    }
  }

  candidate = humanizeSlug(candidate);
  candidate = candidate.replace(/\s{2,}/g, " ").trim();
  return ellipsize(candidate || rawId || "model", maxLen);
}

/** Composer trigger — tighter budget. */
export function prettyModelTrigger(
  label: string | null | undefined,
  id: string | null | undefined,
): string {
  return prettyModelLabel(
    { id: id || "", label: label || id || "model", description: undefined },
    { maxLen: 22 },
  );
}

/** Full tooltip text (id + description). */
export function modelTooltip(model: Pick<ModelDef, "id" | "label" | "description">): string {
  return [model.label || model.id, model.description, model.id].filter(Boolean).join("\n");
}
