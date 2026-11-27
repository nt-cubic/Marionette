/**
 * UsageOverrides - the user-owned pricing / alias / filter layer of the Token
 * usage page. It lives inside the expandable detailed-analysis section so the
 * first screen stays the T3-style summary.
 *
 * Custom prices are deliberately labelled as estimates. They only price the
 * tokens the agent did not report itself; an agent-reported USD cost remains
 * authoritative (see `costOfModelUsage`).
 */
import { useMemo } from "react";
import { useT } from "../../lib/i18n";
import type { UsageInsights } from "../../lib/usageInsights";
import {
  emptyUsageOverrides,
  hasAnyOverride,
  type CustomPrice,
  type UsageOverrides as UsageOverridesState,
} from "../../lib/usageOverrides";
import { SectionHead, SourceTag } from "./primitives";

type FilterMode = "all" | "include" | "exclude";

const EMPTY_PRICE: CustomPrice = { input: null, output: null, cacheRead: null, cacheWrite: null };

function priceOf(overrides: UsageOverridesState, modelId: string): CustomPrice {
  const direct = overrides.prices[modelId];
  if (direct) return direct;
  const key = Object.keys(overrides.prices).find((k) => k.toLowerCase() === modelId.toLowerCase());
  return (key ? overrides.prices[key] : undefined) ?? EMPTY_PRICE;
}

function aliasOf(overrides: UsageOverridesState, modelId: string): string {
  const direct = overrides.aliases[modelId];
  if (direct != null) return direct;
  const key = Object.keys(overrides.aliases).find((k) => k.toLowerCase() === modelId.toLowerCase());
  return key ? overrides.aliases[key] ?? "" : "";
}

function modeOf(overrides: UsageOverridesState, modelId: string): FilterMode {
  const has = (list: readonly string[]) => list.some((id) => id.toLowerCase() === modelId.toLowerCase());
  if (has(overrides.filter.exclude)) return "exclude";
  if (has(overrides.filter.include)) return "include";
  return "all";
}

function withPrice(
  overrides: UsageOverridesState,
  modelId: string,
  field: keyof CustomPrice,
  value: number | null,
): UsageOverridesState {
  const current = { ...priceOf(overrides, modelId), [field]: value };
  const prices = { ...overrides.prices };
  for (const key of Object.keys(prices)) {
    if (key.toLowerCase() === modelId.toLowerCase()) delete prices[key];
  }
  if (
    current.input == null &&
    current.output == null &&
    current.cacheRead == null &&
    current.cacheWrite == null
  ) {
    return { ...overrides, prices };
  }
  prices[modelId] = current;
  return { ...overrides, prices };
}

function withAlias(overrides: UsageOverridesState, modelId: string, raw: string): UsageOverridesState {
  const target = raw.trim();
  const aliases = { ...overrides.aliases };
  for (const key of Object.keys(aliases)) {
    if (key.toLowerCase() === modelId.toLowerCase()) delete aliases[key];
  }
  if (target && target !== modelId) aliases[modelId] = target;
  return { ...overrides, aliases };
}

function withMode(overrides: UsageOverridesState, modelId: string, mode: FilterMode): UsageOverridesState {
  const include = overrides.filter.include.filter((id) => id.toLowerCase() !== modelId.toLowerCase());
  const exclude = overrides.filter.exclude.filter((id) => id.toLowerCase() !== modelId.toLowerCase());
  if (mode === "include") include.push(modelId);
  if (mode === "exclude") exclude.push(modelId);
  return { ...overrides, filter: { include, exclude } };
}

function PriceInput({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number | null;
  onChange: (value: number | null) => void;
}) {
  return (
    <label className="uv2-ov__price">
      <span>{label}</span>
      <input
        type="number"
        min="0"
        step="0.0001"
        value={value ?? ""}
        placeholder="\u2014"
        onChange={(event) => {
          const raw = event.target.value.trim();
          if (raw === "") {
            onChange(null);
            return;
          }
          const next = Number(raw);
          onChange(Number.isFinite(next) && next >= 0 ? next : null);
        }}
      />
    </label>
  );
}

export function UsageOverrides({
  ins,
  overrides,
  onChange,
}: {
  ins: UsageInsights;
  overrides: UsageOverridesState;
  onChange: (next: UsageOverridesState) => void;
}) {
  const t = useT();

  const modelIds = useMemo(() => {
    const set = new Set<string>();
    for (const id of ins.allModels) set.add(id);
    for (const id of Object.keys(overrides.prices)) set.add(id);
    for (const id of Object.keys(overrides.aliases)) set.add(id);
    for (const id of overrides.filter.include) set.add(id);
    for (const id of overrides.filter.exclude) set.add(id);
    return [...set].sort((a, b) => a.localeCompare(b));
  }, [ins, overrides]);

  const customCount = Object.keys(overrides.prices).length;

  return (
    <section className="uv2-sec uv2-ov">
      <SectionHead
        title={t("usageV2.ov.title")}
        meta={t("usageV2.ov.meta")}
        right={
          hasAnyOverride(overrides) ? (
            <button type="button" className="uv2-ghost" onClick={() => onChange(emptyUsageOverrides())}>
              {t("usageV2.ov.reset")}
            </button>
          ) : undefined
        }
      />
      <p className="uv2-ov__note">
        <SourceTag source="custom" />
        {t("usageV2.ov.note")}
      </p>
      {modelIds.length === 0 ? (
        <p className="uv2-ov__empty">{t("usageV2.ov.empty")}</p>
      ) : (
        <div className="uv2-ov__list">
          <div className="uv2-ov__th">
            <span>{t("usageV2.ov.col.model")}</span>
            <span>{t("usageV2.ov.col.alias")}</span>
            <span>{t("usageV2.ov.col.input")}</span>
            <span>{t("usageV2.ov.col.output")}</span>
            <span>{t("usageV2.ov.col.cacheRead")}</span>
            <span>{t("usageV2.ov.col.cacheWrite")}</span>
            <span>{t("usageV2.ov.col.filter")}</span>
          </div>
          {modelIds.map((modelId) => {
            const price = priceOf(overrides, modelId);
            const alias = aliasOf(overrides, modelId);
            return (
              <div className="uv2-ov__row" key={modelId}>
                <span className="uv2-ov__model" title={modelId}>
                  <b>{modelId}</b>
                  {customCount > 0 && price !== EMPTY_PRICE && <SourceTag source="custom" />}
                </span>
                <input
                  className="uv2-ov__alias"
                  type="text"
                  value={alias}
                  placeholder={t("usageV2.ov.aliasPlaceholder")}
                  aria-label={t("usageV2.ov.aliasAria", { model: modelId })}
                  onChange={(event) => onChange(withAlias(overrides, modelId, event.target.value))}
                />
                <PriceInput
                  label={t("usageV2.ov.col.input")}
                  value={price.input}
                  onChange={(value) => onChange(withPrice(overrides, modelId, "input", value))}
                />
                <PriceInput
                  label={t("usageV2.ov.col.output")}
                  value={price.output}
                  onChange={(value) => onChange(withPrice(overrides, modelId, "output", value))}
                />
                <PriceInput
                  label={t("usageV2.ov.col.cacheRead")}
                  value={price.cacheRead}
                  onChange={(value) => onChange(withPrice(overrides, modelId, "cacheRead", value))}
                />
                <PriceInput
                  label={t("usageV2.ov.col.cacheWrite")}
                  value={price.cacheWrite}
                  onChange={(value) => onChange(withPrice(overrides, modelId, "cacheWrite", value))}
                />
                <select
                  className="uv2-ov__filter"
                  value={modeOf(overrides, modelId)}
                  aria-label={t("usageV2.ov.filterAria", { model: modelId })}
                  onChange={(event) => onChange(withMode(overrides, modelId, event.target.value as FilterMode))}
                >
                  <option value="all">{t("usageV2.ov.filter.all")}</option>
                  <option value="include">{t("usageV2.ov.filter.include")}</option>
                  <option value="exclude">{t("usageV2.ov.filter.exclude")}</option>
                </select>
              </div>
            );
          })}
        </div>
      )}
      <p className="uv2-ov__filterNote">{t("usageV2.ov.filterNote")}</p>
    </section>
  );
}

