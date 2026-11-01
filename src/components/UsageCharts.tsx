import {
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { formatTokenCount } from "../lib/sessionStats";
import { hourRampStep } from "../lib/usageLedger";

/**
 * Token 用量页的手绘图表。骨架学 codeg `token-usage/charts.tsx`:
 * 零依赖、柱宽封顶、两色语言(强调色=缓存,墨色=新算)、身份靠文字。
 * 表面走 docs/ui-language.md(3px 角、1px 边、无阴影、只动 transform/opacity)。
 */

export const U_ACCENT = "var(--u-accent)";
export const U_INK = "var(--u-ink)";
export const U_INK_2 = "var(--u-ink-2)";
export const U_INK_3 = "var(--u-ink-3)";
export const U_FAINT = "var(--text-faint)";

const TREND_HEIGHT = 176;
const TREND_PAD_TOP = 14;
const TREND_PAD_BOTTOM = 20;
const GRID_LINES = 4;
const MAX_BAR_WIDTH = 24;

function useElementWidth<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      setWidth(entries[0]?.contentRect.width ?? 0);
    });
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  return [ref, width] as const;
}

function roundedTopPath(x: number, y: number, w: number, h: number, r: number): string {
  const rr = Math.max(0, Math.min(r, w / 2, h));
  return [
    `M${x},${y + h}`,
    `V${y + rr}`,
    `Q${x},${y} ${x + rr},${y}`,
    `H${x + w - rr}`,
    `Q${x + w},${y} ${x + w},${y + rr}`,
    `V${y + h}`,
    "Z",
  ].join(" ");
}

export type TrendDatum = {
  key: string;
  label: string;
  tooltipLabel?: string;
  cache: number;
  fresh: number;
  detail: { label: string; value: string }[];
};

export function TrendChart({
  data,
  label,
  cacheLabel,
  freshLabel,
  emptyLabel,
}: {
  data: TrendDatum[];
  label: string;
  cacheLabel: string;
  freshLabel: string;
  emptyLabel: string;
}) {
  const [ref, width] = useElementWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const max = useMemo(
    () => data.reduce((m, d) => Math.max(m, d.cache + d.fresh), 0),
    [data],
  );
  const plotHeight = TREND_HEIGHT - TREND_PAD_TOP - TREND_PAD_BOTTOM;
  const slot = data.length > 0 && width > 0 ? width / data.length : 0;

  const marks = useMemo(() => {
    if (slot <= 0 || max <= 0) return null;
    const gap = slot > 5 ? 2 : 0;
    const barWidth = Math.min(MAX_BAR_WIDTH, Math.max(1, slot - gap));
    const radius = Math.min(3, barWidth / 2);
    const baseline = TREND_PAD_TOP + plotHeight;
    return (
      <g>
        {data.map((d, i) => {
          const total = d.cache + d.fresh;
          if (total <= 0) return null;
          const totalH = Math.max(2, (total / max) * plotHeight);
          const x = i * slot + (slot - barWidth) / 2;
          const freshH = (d.fresh / total) * totalH;
          const cacheH = totalH - freshH;
          const seam = freshH >= 4 && cacheH >= 2 ? 2 : 0;
          const parts: ReactNode[] = [];
          if (cacheH > 0.5) {
            parts.push(
              freshH > 0.5 ? (
                <rect
                  key="c"
                  x={x}
                  y={baseline - cacheH}
                  width={barWidth}
                  height={cacheH}
                  fill={U_ACCENT}
                />
              ) : (
                <path
                  key="c"
                  d={roundedTopPath(x, baseline - cacheH, barWidth, cacheH, radius)}
                  fill={U_ACCENT}
                />
              ),
            );
          }
          if (freshH > 0.5) {
            parts.push(
              <path
                key="f"
                d={roundedTopPath(x, baseline - totalH, barWidth, Math.max(1, freshH - seam), radius)}
                fill={U_INK}
              />,
            );
          }
          return <g key={d.key}>{parts}</g>;
        })}
      </g>
    );
  }, [data, slot, max, plotHeight]);

  if (data.length === 0) {
    return <div className="usage-trend usage-trend--empty">{emptyLabel}</div>;
  }

  const hovered = hover != null ? data[hover] : null;
  const moveTo = (index: number) => {
    setHover(Math.max(0, Math.min(data.length - 1, index)));
  };

  return (
    <div ref={ref} className="usage-trend">
      {width > 0 && (
        <svg
          width={width}
          height={TREND_HEIGHT}
          role="img"
          aria-label={label}
          className="usage-trend__svg"
          tabIndex={0}
          onMouseLeave={() => setHover(null)}
          onMouseMove={(event) => {
            const rect = event.currentTarget.getBoundingClientRect();
            const index = Math.floor((event.clientX - rect.left) / slot);
            setHover(index >= 0 && index < data.length ? index : null);
          }}
          onKeyDown={(event) => {
            const current = hover ?? data.length - 1;
            const moves: Record<string, number> = {
              ArrowLeft: current - 1,
              ArrowRight: current + 1,
              Home: 0,
              End: data.length - 1,
            };
            if (event.key in moves) {
              event.preventDefault();
              moveTo(moves[event.key]!);
            } else if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              setHover(null);
            }
          }}
        >
          {Array.from({ length: GRID_LINES + 1 }, (_, i) => {
            const y = TREND_PAD_TOP + (plotHeight * i) / GRID_LINES;
            return (
              <line
                key={i}
                x1={0}
                x2={width}
                y1={y}
                y2={y}
                className="usage-trend__grid"
                opacity={i === GRID_LINES ? 0.8 : 0.35}
              />
            );
          })}
          {hover != null && (
            <rect
              x={hover * slot}
              y={TREND_PAD_TOP}
              width={slot}
              height={plotHeight}
              fill="var(--u-accent-soft)"
              opacity={0.7}
            />
          )}
          {marks}
          <text x={0} y={TREND_HEIGHT - 5} className="usage-trend__axis">
            {data[0]?.label}
          </text>
          {data.length > 1 && (
            <text x={width} y={TREND_HEIGHT - 5} textAnchor="end" className="usage-trend__axis">
              {data[data.length - 1]?.label}
            </text>
          )}
          <text x={0} y={TREND_PAD_TOP - 4} className="usage-trend__axis">
            {formatTokenCount(max)}
          </text>
        </svg>
      )}
      {hover != null && hovered && (
        <div
          className="usage-tip"
          style={{
            transform: `translateX(${Math.min(
              Math.max(0, (hover + 0.5) * slot - 80),
              Math.max(0, width - 168),
            )}px)`,
          }}
        >
          <div className="usage-tip__head">
            <span>{hovered.tooltipLabel ?? hovered.label}</span>
            <span className="usage-tip__value">{formatTokenCount(hovered.cache + hovered.fresh)}</span>
          </div>
          <div className="usage-tip__row">
            <span>
              <i style={{ background: U_ACCENT }} />
              {cacheLabel}
            </span>
            <span>{formatTokenCount(hovered.cache)}</span>
          </div>
          <div className="usage-tip__row">
            <span>
              <i style={{ background: U_INK }} />
              {freshLabel}
            </span>
            <span>{formatTokenCount(hovered.fresh)}</span>
          </div>
          {hovered.detail.map((line) => (
            <div key={line.label} className="usage-tip__row">
              <span>{line.label}</span>
              <span>{line.value}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export function RingMeter({
  ratio,
  valueText,
  caption,
  size = 96,
  thickness = 10,
}: {
  ratio: number;
  valueText: string;
  caption: string;
  size?: number;
  thickness?: number;
}) {
  const clamped = Math.min(1, Math.max(0, ratio));
  const r = (size - thickness) / 2;
  const circumference = 2 * Math.PI * r;
  return (
    <div
      className="usage-ring"
      style={{ width: size, height: size }}
      role="img"
      aria-label={`${caption} ${valueText}`}
    >
      <svg width={size} height={size} className="usage-ring__svg" aria-hidden>
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          stroke="var(--u-accent-soft)"
          strokeWidth={thickness}
        />
        {clamped > 0 && (
          <circle
            cx={size / 2}
            cy={size / 2}
            r={r}
            fill="none"
            stroke={U_ACCENT}
            strokeWidth={thickness}
            strokeLinecap="round"
            strokeDasharray={circumference}
            strokeDashoffset={circumference * (1 - clamped)}
          />
        )}
      </svg>
      <div className="usage-ring__center">
        <span className="usage-ring__value">{valueText}</span>
        <span className="usage-ring__caption">{caption}</span>
      </div>
    </div>
  );
}

export type RankedDatum = {
  key: string;
  label: string;
  value: number;
  share: number | null;
  hint?: string;
  color?: string;
};

export function RankedBars({
  data,
  emptyLabel,
  showRank = false,
}: {
  data: RankedDatum[];
  emptyLabel: string;
  showRank?: boolean;
}) {
  const max = data.reduce((m, d) => Math.max(m, d.value), 0);
  if (data.length === 0) {
    return <p className="usage-rank__empty">{emptyLabel}</p>;
  }
  return (
    <ul className="usage-rank">
      {data.map((d, i) => {
        const pct = max > 0 ? (d.value / max) * 100 : 0;
        const fill = d.color ?? U_ACCENT;
        return (
          <li key={d.key} className="usage-rank__item">
            <div className="usage-rank__line">
              <span className="usage-rank__label">
                {showRank && (
                  <span className="usage-rank__n" aria-hidden>
                    {String(i + 1).padStart(2, "0")}
                  </span>
                )}
                <span className="usage-rank__name">{d.label}</span>
              </span>
              {d.hint ? <span className="usage-rank__hint">{d.hint}</span> : null}
              <span className="usage-rank__value">{formatTokenCount(d.value)}</span>
              {d.share != null && (
                <span className="usage-rank__pct">{`${(d.share * 100).toFixed(1)}%`}</span>
              )}
            </div>
            <div
              className={`usage-rank__track${showRank ? " is-indented" : ""}`}
            >
              <i
                style={
                  {
                    width: `${Math.max(pct, d.value > 0 ? 1.5 : 0)}%`,
                    background: fill,
                  } as CSSProperties
                }
              />
            </div>
          </li>
        );
      })}
    </ul>
  );
}

const RAMP = [
  "var(--u-seq-1)",
  "var(--u-seq-2)",
  "var(--u-seq-3)",
  "var(--u-seq-4)",
  "var(--u-seq-5)",
  "var(--u-seq-6)",
  "var(--u-seq-7)",
] as const;

/** 平方根分档:一次通宵不会把其余格子全压成最浅一档。0 不进 ramp。 */
export function rampStep(value: number, max: number): number | null {
  return hourRampStep(value, max, RAMP.length);
}

const HOUR_TICKS = [0, 6, 12, 18] as const;

type HeatHover = {
  weekday: number;
  hour: number;
  value: number;
  x: number;
  y: number;
  w: number;
  h: number;
  containerW: number;
};

export function HourHeatmap({
  matrix,
  weekdayLabels,
  formatAria,
  legendLess,
  legendMore,
  formatValue,
}: {
  matrix: number[][];
  weekdayLabels: string[];
  formatAria: (weekday: number, hour: number, value: number) => string;
  legendLess: string;
  legendMore: string;
  formatValue: (value: number) => string;
}) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [hover, setHover] = useState<HeatHover | null>(null);
  const max = useMemo(
    () => matrix.reduce((m, row) => Math.max(m, ...row), 0),
    [matrix],
  );

  const onEnter = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>, weekday: number, hour: number, value: number) => {
      const wrap = wrapRef.current;
      if (!wrap) return;
      const cell = event.currentTarget.getBoundingClientRect();
      const box = wrap.getBoundingClientRect();
      setHover({
        weekday,
        hour,
        value,
        x: cell.left - box.left,
        y: cell.top - box.top,
        w: cell.width,
        h: cell.height,
        containerW: box.width,
      });
    },
    [],
  );

  return (
    <div ref={wrapRef} className="usage-rhythm">
      <div className="usage-rhythm__grid" onMouseLeave={() => setHover(null)}>
        <div className="usage-rhythm__hours">
          {HOUR_TICKS.map((h) => (
            <span key={h} style={{ gridColumn: `${h + 1} / span 2` }}>
              {h}
            </span>
          ))}
          <span className="usage-rhythm__hour-end" style={{ gridColumn: "23 / span 2" }}>
            23
          </span>
        </div>
        {matrix.map((row, weekday) => (
          <div key={weekday} className="usage-rhythm__row">
            <span className="usage-rhythm__wd">
              {weekday % 2 === 0 ? weekdayLabels[weekday] : ""}
            </span>
            <div className="usage-rhythm__cells">
              {row.map((value, hour) => {
                const step = rampStep(value, max);
                return (
                  <div
                    key={hour}
                    role="img"
                    aria-label={formatAria(weekday, hour, value)}
                    className={`usage-rhythm__cell${step == null ? " is-empty" : ""}`}
                    style={step == null ? undefined : { background: RAMP[step] }}
                    onMouseEnter={(event) => onEnter(event, weekday, hour, value)}
                  />
                );
              })}
            </div>
          </div>
        ))}
      </div>
      <div className="usage-rhythm__legend">
        <span>{legendLess}</span>
        <span className="usage-rhythm__swatch is-empty" />
        {RAMP.map((color) => (
          <span key={color} className="usage-rhythm__swatch" style={{ background: color }} />
        ))}
        <span>{legendMore}</span>
      </div>
      {hover && (
        <>
          <div
            aria-hidden
            className="usage-rhythm__ring"
            style={{
              transform: `translate(${hover.x - 1}px, ${hover.y - 1}px)`,
              width: hover.w + 2,
              height: hover.h + 2,
            }}
          />
          <div
            className="usage-tip usage-tip--heat"
            style={{
              left: Math.min(Math.max(hover.x + hover.w / 2, 56), Math.max(56, hover.containerW - 56)),
              top: hover.y - 6,
            }}
          >
            <div className="usage-tip__head">
              {weekdayLabels[hover.weekday]} {String(hover.hour).padStart(2, "0")}:00
            </div>
            <div className="usage-tip__value">{formatValue(hover.value)}</div>
          </div>
        </>
      )}
    </div>
  );
}
