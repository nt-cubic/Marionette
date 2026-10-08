import type { ModelDef } from "./types";

/**
 * Display-only cleanup for ACP labels — model names, their menu group, and
 * effort tiers. Wire ids stay untouched for set_model / prefs.
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

/** Trailing parenthetical of a display name: "… (Teamo)" → "Teamo". */
function trailingTag(label: string): string | null {
  const m = /\(([^()]*)\)\s*$/.exec(label.trim());
  return m ? m[1].trim() : null;
}

/**
 * Backend tags users put in a catalog entry's `name`. Grok publishes a flat
 * catalog (`modelId` + `name`, no endpoint), so these tags are the only signal
 * that says *where* a model runs; they beat the family prefix below.
 */
const BACKEND_TAGS: Array<{ match: RegExp; group: string }> = [
  { match: /^teamo\b/i, group: "Team Router" },
  { match: /^local\b/i, group: "Local" },
];

/** Backend words the group header already prints, matched inside a tag. */
const REDUNDANT_TAG_PARTS = /^(teamo|local|official)\b/i;

/** Family / vendor prefixes, checked against the catalog key, then the label. */
const FAMILY_GROUPS: Array<{ match: RegExp; group: string }> = [
  { match: /^teamo\b/i, group: "Team Router" },
  { match: /^opencode\b/i, group: "OpenCode" },
  { match: /^openrouter\b/i, group: "OpenRouter" },
  { match: /^deepseek\b/i, group: "DeepSeek" },
  { match: /^(xai|grok)\b/i, group: "xAI" },
  { match: /^(glm|zai|z\.ai)\b/i, group: "Z.AI" },
  { match: /^(claude|anthropic)\b/i, group: "Claude" },
  { match: /^(gpt|openai)\b/i, group: "OpenAI" },
  { match: /^(gemini|google)\b/i, group: "Google" },
  { match: /^(kimi|moonshot)\b/i, group: "Kimi" },
  { match: /^minimax\b/i, group: "MiniMax" },
  { match: /^(qwen|alibaba)\b/i, group: "Qwen" },
  { match: /^ornith\b/i, group: "Ornith" },
  { match: /^logfare\b/i, group: "Logfare" },
];

/** Claude ACP offers family aliases (`opus`, `sonnet`, `default`) as model ids. */
const CLAUDE_ALIASES = /^(default|best|opusplan|opus|sonnet|haiku|fable)\b/;

function familyGroup(text: string): string | null {
  const hit = FAMILY_GROUPS.find((rule) => rule.match.test(text.trim()));
  return hit ? hit.group : null;
}

/** Provider path prefix of `provider/model` ids and labels. */
function pathPrefix(model: Pick<ModelDef, "id" | "label">): string | null {
  const idPart = model.id.includes("/") ? model.id.split("/")[0] : "";
  const labelPart = model.label.includes("/") ? model.label.split("/")[0] : "";
  return (labelPart || idPart).trim() || null;
}

/**
 * Group header for one model row. A flat catalog (Grok: `modelId` + `name`
 * only) has no endpoint field, so the user's own naming decides: a backend tag
 * in the name wins, then a `provider/model` path, then the family prefix of the
 * catalog key, then the label. Agents that already advertise paths keep working.
 */
export function modelProviderGroup(model: Pick<ModelDef, "id" | "label">): string {
  const label = (model.label || "").trim();
  const id = (model.id || "").trim();

  const tag = trailingTag(label);
  const tagged = tag ? BACKEND_TAGS.find((rule) => rule.match.test(tag)) : undefined;
  if (tagged) return tagged.group;

  const path = pathPrefix(model);
  if (path) return familyGroup(path) ?? path;

  if (CLAUDE_ALIASES.test(id.toLowerCase())) return "Claude";

  return familyGroup(id) ?? familyGroup(label) ?? "Models";
}

/**
 * Menu row label: the display name without the backend words the group header
 * already prints. `claude-opus-4-8 (Teamo)` reads as noise under "Team Router",
 * and `(Official)` under "DeepSeek" — while `(Local, 1M)` keeps its `1M`.
 */
export function modelRowLabel(
  model: Pick<ModelDef, "id" | "label" | "description">,
  maxLen = 40,
): string {
  const label = (model.label || "").trim();
  const tag = trailingTag(label);
  let stripped = label;
  if (tag) {
    const kept = tag
      .split(",")
      .map((part) => part.trim())
      .filter((part) => part && !REDUNDANT_TAG_PARTS.test(part));
    const base = label.replace(/\s*\([^()]*\)\s*$/, "").trim();
    stripped = kept.length > 0 ? `${base} (${kept.join(", ")})` : base;
  }
  return prettyModelLabel({ ...model, label: stripped }, { maxLen });
}

/** Group models by {@link modelProviderGroup}, both axes alphabetical. */
export function groupModelsByProvider(
  models: ModelDef[],
): { provider: string; models: ModelDef[] }[] {
  const map = new Map<string, ModelDef[]>();
  for (const model of models) {
    const provider = modelProviderGroup(model);
    const list = map.get(provider);
    if (list) list.push(model);
    else map.set(provider, [model]);
  }
  return [...map.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([provider, items]) => ({
      provider,
      models: items
        .slice()
        .sort((x, y) => modelRowLabel(x).localeCompare(modelRowLabel(y))),
    }));
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

/**
 * Effort-tier label without the axis name the composer already prints above the
 * row. Agents name the axis themselves ("High Effort"), so rendering that label
 * verbatim under an "EFFORT" header repeats the word on every button.
 *
 * Falls back to the wire id, which is what Rust substitutes when a tier arrives
 * with no label at all.
 */
export function prettyEffortLabel(
  label: string | null | undefined,
  id?: string | null,
): string {
  const raw = (label ?? "").trim() || (id ?? "").trim();
  if (!raw) return "";
  // "Max Effort" → "Max"; a label that is only "Effort" is kept as-is.
  return raw.replace(/\s+effort$/i, "").trim() || raw;
}
