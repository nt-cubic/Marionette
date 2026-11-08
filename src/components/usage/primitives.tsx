/**
 * Shared pieces of the Token usage page v2 (docs/design: canvas board「v2 · Token 用量」).
 * Every section component imports from here; none of them defines its own copy.
 */
import type { ReactNode } from "react";
import { formatDurationCompact, formatTokenCount } from "../../lib/sessionStats";
import { formatUsd } from "../../lib/tokenPrices";
import { useT } from "../../lib/i18n";
import type { Source } from "../../lib/usageInsights";

/** 1.84 亿 / 12.3 万 / 9,812 (zh) · 184M / 123K (en) via the app's own formatter. */
export function fmtTokens(n: number): string {
  return formatTokenCount(Math.round(n));
}

/** 31h 12m · 4.8s · 120ms */
export function fmtMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`;
  if (ms < 3_600_000) return formatDurationCompact(ms);
  // Hours: "31h 12m" reads at a glance; "1872分12秒" does not.
  const minutes = Math.round(ms / 60_000);
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/** 0.941 → "94.1%" */
export function fmtPct(ratio: number | null, digits = 1): string {
  return ratio == null ? "—" : `${(ratio * 100).toFixed(digits)}%`;
}

/** USD list-price conversion; "—" when nothing could be priced. */
export function fmtUsd(usd: number | null): string {
  return usd == null ? "—" : formatUsd(usd);
}

/** +12.4% / −3.0% (null → empty). */
export function fmtDelta(ratio: number | null): string {
  if (ratio == null || !Number.isFinite(ratio)) return "";
  const v = (ratio * 100).toFixed(1);
  return ratio >= 0 ? `↑ ${v}%` : `↓ ${v.replace("-", "")}%`;
}

/** Provenance chip: 实测 / 估算 / 推算 / 折算. */
export function SourceTag({ source }: { source: Source | "priced" | "clock" }) {
  const t = useT();
  return <span className={`uv2-tag uv2-tag--${source}`}>{t(`usageV2.source.${source}`)}</span>;
}

/**
 * Line sparkline with a faint area, last point dotted. `color` is any CSS colour
 * (pass `var(--agent-ink)` for an agent row, `var(--accent)` for totals).
 */
export function Sparkline({
  values,
  color = "var(--accent)",
  width = 132,
  height = 28,
  area = true,
}: {
  values: readonly number[];
  color?: string;
  width?: number;
  height?: number;
  area?: boolean;
}) {
  if (values.length < 2 || values.every((v) => v === 0)) {
    return <svg className="uv2-spark is-empty" width={width} height={height} aria-hidden="true" />;
  }
  const max = Math.max(...values);
  const min = Math.min(...values);
  const pts = values.map((v, i) => {
    const x = (i / (values.length - 1)) * width;
    const y = height - 2 - ((v - min) / (max - min || 1)) * (height - 4);
    return [x, y] as const;
  });
  const line = pts.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
  const last = pts[pts.length - 1]!;
  return (
    <svg className="uv2-spark" width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
      {area && <polygon points={`0,${height} ${line} ${width},${height}`} fill={color} fillOpacity={0.12} />}
      <polyline points={line} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" />
      <circle cx={last[0]} cy={last[1]} r={2.5} fill={color} />
    </svg>
  );
}

/** Section heading row: title, a faint meta line, optional controls on the right. */
export function SectionHead({ title, meta, right }: { title: string; meta?: ReactNode; right?: ReactNode }) {
  return (
    <div className="uv2-sh">
      <h2>{title}</h2>
      {meta != null && <span className="uv2-sh__meta">{meta}</span>}
      {right != null && <span className="uv2-sh__right">{right}</span>}
    </div>
  );
}

/** Text-only segmented control (underline on the active item), as in the page top bar. */
export function Seg<T extends string | number>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (v: T) => void;
  label: string;
}) {
  return (
    <span className="uv2-seg" role="tablist" aria-label={label}>
      {options.map((o) => (
        <button
          key={String(o.value)}
          type="button"
          role="tab"
          aria-selected={o.value === value}
          className={o.value === value ? "is-on" : undefined}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </span>
  );
}
