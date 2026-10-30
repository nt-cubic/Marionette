import { t } from "./i18n";

/**
 * Schema for ` ```spec ` fences (generative UI).
 *
 * A spec fence is a JSON object the assistant emits to ask Marionette for an
 * interactive card instead of prose. Parsing runs three guards before any
 * render (research-codeg.md B4, adopted):
 *
 * 1. JSON.parse in try/catch — malformed / streaming-incomplete fences get an
 *    error card with the raw text, never a crash;
 * 2. prototype-pollution key blacklist + depth cap on the whole tree —
 *    `JSON.parse` *can* create `__proto__` own properties, so the walk is not
 *    paranoia; deep nesting is the recursion bomb;
 * 3. strict shape validation per type with size caps — the card renders user
 *    and agent facing, so limits keep a broken spec from eating the screen.
 *
 * The render side adds a fourth guard (React error boundary). Pure module —
 * no React imports, scripts/verify-rich-markdown.mts runs it directly.
 */

export const SPEC_ACTION_EVENT = "marionette:spec-action";

/** ```spec bodies larger than this refuse to parse (codeg brief frame parity). */
export const SPEC_RAW_LIMIT = 131072;

const MAX_DEPTH = 64;
const FORBIDDEN_KEYS = new Set(["__proto__", "constructor", "prototype"]);

const LIMITS = {
  title: 300,
  fieldName: 120,
  fieldValue: 8000,
  body: 20000,
  columns: 32,
  columnLabel: 200,
  cellText: 2000,
  rows: 200,
  fields: 40,
  steps: 100,
  stepLabel: 500,
  note: 1000,
  series: 8,
  seriesName: 200,
  points: 200,
  pointLabel: 200,
  actions: 6,
  actionLabel: 160,
  actionPrompt: 8000,
} as const;

const TONES: readonly SpecTone[] = ["ok", "warn", "err", "dim"];
const STATES: readonly SpecState[] = ["done", "active", "todo", "error"];

export type SpecTone = "ok" | "warn" | "err" | "dim";
export type SpecState = "done" | "active" | "todo" | "error";
export type SpecAction = { label: string; prompt: string };
export type SpecField = { name: string; value: string; hint?: string };
export type SpecCell = string | { text: string; tone?: SpecTone };
export type SpecPoint = { label?: string; value: number };

export type SpecSpec =
  | {
      type: "card";
      title?: string;
      fields: SpecField[];
      body?: string;
      actions?: SpecAction[];
    }
  | {
      type: "table";
      title?: string;
      columns: string[];
      rows: SpecCell[][];
      actions?: SpecAction[];
    }
  | {
      type: "steps";
      title?: string;
      items: { label: string; state: SpecState; note?: string }[];
      actions?: SpecAction[];
    }
  | {
      type: "chart";
      chart: "bar" | "line" | "pie";
      title?: string;
      series: { name?: string; points: SpecPoint[] }[];
      actions?: SpecAction[];
    };

export type SpecParseResult = { ok: true; spec: SpecSpec } | { ok: false; error: string };

function fail(error: string): SpecParseResult {
  return { ok: false, error };
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function checkTree(value: unknown, depth: number): string | null {
  if (depth > MAX_DEPTH) return t("spec.tooDeep", { n: MAX_DEPTH });
  if (Array.isArray(value)) {
    for (const item of value) {
      const err = checkTree(item, depth + 1);
      if (err) return err;
    }
    return null;
  }
  if (isPlainObject(value)) {
    for (const key of Object.keys(value)) {
      if (FORBIDDEN_KEYS.has(key)) return t("spec.illegalKey", { key });
      const err = checkTree(value[key], depth + 1);
      if (err) return err;
    }
  }
  return null;
}

/** Optional string field: `{}` when absent, `error` when not a string / too long. */
function readStr(
  obj: Record<string, unknown>,
  key: string,
  max: number,
): { value?: string; error?: string } {
  const value = obj[key];
  if (value === undefined || value === null) return {};
  if (typeof value !== "string") return { error: t("spec.mustBeString", { key }) };
  if (value.length > max) return { error: t("spec.tooLong", { key, max }) };
  return { value };
}

/**
 * Required string field with a non-empty constraint. Proper discriminated
 * union ({ok}) — optional-property unions don't narrow on truthiness and
 * leaked `string | undefined` into the strict call sites.
 */
function requireStr(
  obj: Record<string, unknown>,
  key: string,
  max: number,
  what: string,
): { ok: true; value: string } | { ok: false; error: string } {
  const field = readStr(obj, key, max);
  if (field.error) return { ok: false, error: field.error };
  if (field.value === undefined) return { ok: false, error: t("spec.missingStrField", { what, key }) };
  if (!field.value.trim()) return { ok: false, error: t("spec.strFieldEmpty", { what, key }) };
  return { ok: true, value: field.value };
}

function readActions(obj: Record<string, unknown>): SpecAction[] | string {
  const raw = obj.actions;
  if (raw === undefined || raw === null) return [];
  if (!Array.isArray(raw) || raw.length === 0) return t("spec.actionsShape");
  if (raw.length > LIMITS.actions) return t("spec.actionsTooMany", { n: LIMITS.actions });
  const out: SpecAction[] = [];
  for (const item of raw) {
    if (!isPlainObject(item)) return t("spec.actionsItemObject");
    const label = requireStr(item, "label", LIMITS.actionLabel, "actions[]");
    if (!label.ok) return label.error;
    const prompt = requireStr(item, "prompt", LIMITS.actionPrompt, "actions[]");
    if (!prompt.ok) return prompt.error;
    out.push({ label: label.value, prompt: prompt.value });
  }
  return out;
}

function readTitle(obj: Record<string, unknown>): string | undefined | "error" {
  if (obj.title === undefined || obj.title === null) return undefined;
  const field = readStr(obj, "title", LIMITS.title);
  return field.error ? "error" : field.value;
}

function validateSpec(obj: Record<string, unknown>): SpecParseResult {
  const type = obj.type;
  if (typeof type !== "string") return fail(t("spec.missingType"));

  const title = readTitle(obj);
  if (title === "error") return fail(t("spec.titleShape"));
  const titleField = title === undefined ? {} : { title };
  const actions = readActions(obj);
  if (typeof actions === "string") return fail(actions);
  const actionField = actions.length ? { actions } : {};

  if (type === "card") {
    if (!Array.isArray(obj.fields) || obj.fields.length === 0)
      return fail(t("spec.cardNeedsFields"));
    if (obj.fields.length > LIMITS.fields) return fail(t("spec.fieldsTooMany", { n: LIMITS.fields }));
    const bodyField = readStr(obj, "body", LIMITS.body);
    if (bodyField.error) return fail(bodyField.error);
    const fields: SpecField[] = [];
    for (const item of obj.fields) {
      if (!isPlainObject(item)) return fail(t("spec.fieldsItemObject"));
      const name = requireStr(item, "name", LIMITS.fieldName, "fields[]");
      if (!name.ok) return fail(name.error);
      const value = requireStr(item, "value", LIMITS.fieldValue, "fields[]");
      if (!value.ok) return fail(value.error);
      const hint = readStr(item, "hint", LIMITS.note);
      if (hint.error) return fail(hint.error);
      fields.push(hint.value ? { name: name.value, value: value.value, hint: hint.value } : { name: name.value, value: value.value });
    }
    return {
      ok: true,
      spec: { type: "card", ...titleField, fields, ...(bodyField.value ? { body: bodyField.value } : {}), ...actionField },
    };
  }

  if (type === "table") {
    if (!Array.isArray(obj.columns) || obj.columns.length === 0)
      return fail(t("spec.tableNeedsColumns"));
    if (obj.columns.length > LIMITS.columns) return fail(t("spec.columnsTooMany", { n: LIMITS.columns }));
    const columns: string[] = [];
    for (const col of obj.columns) {
      if (typeof col !== "string" || !col.trim()) return fail(t("spec.columnsItemString"));
      if (col.length > LIMITS.columnLabel) return fail(t("spec.columnLabelTooLong", { n: LIMITS.columnLabel }));
      columns.push(col);
    }
    if (!Array.isArray(obj.rows) || obj.rows.length === 0) return fail(t("spec.tableNeedsRows"));
    if (obj.rows.length > LIMITS.rows) return fail(t("spec.rowsTooMany", { n: LIMITS.rows }));
    const rows: SpecCell[][] = [];
    for (const row of obj.rows) {
      if (!Array.isArray(row)) return fail(t("spec.rowsItemArray"));
      if (row.length !== columns.length)
        return fail(t("spec.rowWidthMismatch", { width: row.length, columns: columns.length }));
      const cells: SpecCell[] = [];
      for (const cell of row) {
        if (typeof cell === "string") {
          if (cell.length > LIMITS.cellText) return fail(t("spec.cellTooLong", { n: LIMITS.cellText }));
          cells.push(cell);
          continue;
        }
        if (isPlainObject(cell) && typeof cell.text === "string") {
          if (cell.text.length > LIMITS.cellText) return fail(t("spec.cellTooLong", { n: LIMITS.cellText }));
          if (cell.tone !== undefined && !TONES.includes(cell.tone as SpecTone))
            return fail(t("spec.cellToneShape"));
          cells.push(cell.tone ? { text: cell.text, tone: cell.tone as SpecTone } : { text: cell.text });
          continue;
        }
        return fail(t("spec.cellShape"));
      }
      rows.push(cells);
    }
    return { ok: true, spec: { type: "table", ...titleField, columns, rows, ...actionField } };
  }

  if (type === "steps") {
    if (!Array.isArray(obj.items) || obj.items.length === 0)
      return fail(t("spec.stepsNeedsItems"));
    if (obj.items.length > LIMITS.steps) return fail(t("spec.itemsTooMany", { n: LIMITS.steps }));
    const items: { label: string; state: SpecState; note?: string }[] = [];
    for (const item of obj.items) {
      if (!isPlainObject(item)) return fail(t("spec.itemsItemObject"));
      const label = requireStr(item, "label", LIMITS.stepLabel, "items[]");
      if (!label.ok) return fail(label.error);
      if (!STATES.includes(item.state as SpecState))
        return fail(t("spec.stateShape"));
      const note = readStr(item, "note", LIMITS.note);
      if (note.error) return fail(note.error);
      items.push(
        note.value
          ? { label: label.value, state: item.state as SpecState, note: note.value }
          : { label: label.value, state: item.state as SpecState },
      );
    }
    return { ok: true, spec: { type: "steps", ...titleField, items, ...actionField } };
  }

  if (type === "chart") {
    const chart = obj.chart;
    if (!["bar", "line", "pie"].includes(chart as string))
      return fail(t("spec.chartNeedsType"));
    if (!Array.isArray(obj.series) || obj.series.length === 0) return fail(t("spec.chartNeedsSeries"));
    if (obj.series.length > LIMITS.series) return fail(t("spec.seriesTooMany", { n: LIMITS.series }));
    const series: { name?: string; points: SpecPoint[] }[] = [];
    for (const group of obj.series) {
      if (!isPlainObject(group) || !Array.isArray(group.points) || group.points.length === 0)
        return fail(t("spec.seriesNeedsPoints"));
      if (group.points.length > LIMITS.points) return fail(t("spec.pointsTooMany", { n: LIMITS.points }));
      const name = readStr(group, "name", LIMITS.seriesName);
      if (name.error) return fail(name.error);
      const points: SpecPoint[] = [];
      for (const point of group.points) {
        if (!isPlainObject(point)) return fail(t("spec.pointShape"));
        if (typeof point.value !== "number" || !Number.isFinite(point.value))
          return fail(t("spec.pointValueNumber"));
        if (point.label === undefined) {
          points.push({ value: point.value });
          continue;
        }
        if (typeof point.label !== "string" || point.label.length > LIMITS.pointLabel)
          return fail(t("spec.pointLabelTooLong", { n: LIMITS.pointLabel }));
        points.push({ label: point.label, value: point.value });
      }
      series.push(name.value ? { name: name.value, points } : { points });
    }
    return {
      ok: true,
      spec: { type: "chart", chart: chart as "bar" | "line" | "pie", ...titleField, series, ...actionField },
    };
  }

  return fail(t("spec.unknownType", { type }));
}

/** Full guarded parse: JSON → pollution/depth walk → shape validation. */
export function parseSpecSpec(raw: string): SpecParseResult {
  if (raw.length > SPEC_RAW_LIMIT) {
    return fail(t("spec.rawTooLarge", { limit: SPEC_RAW_LIMIT, current: raw.length }));
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    return fail(t("spec.invalidJson", { message: (error as Error).message }));
  }
  if (!isPlainObject(parsed)) return fail(t("spec.mustBeObject"));
  const treeError = checkTree(parsed, 0);
  if (treeError) return fail(treeError);
  return validateSpec(parsed);
}
