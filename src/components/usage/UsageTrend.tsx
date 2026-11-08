/**
 * UsageTrend — the daily stacked-bar band of the Token usage page v2 (task A):
 * four token buckets per day/week column, a dashed list-price cost overlay
 * and a hover / keyboard callout per column.
 */
import { useEffect, useRef, useState } from "react";
import { useT } from "../../lib/i18n";
import type { DayPoint, UsageInsights } from "../../lib/usageInsights";
import { SectionHead, Seg, fmtTokens, fmtUsd } from "./primitives";

const H = 170;
const GAP = 8;

type Grain = "day" | "week";

type Period = {
  label: string;
  cached: number;
  cacheCreation: number;
  input: number;
  output: number;
  total: number;
  usd: number | null;
  turns: number;
};

function dayPeriods(daily: readonly DayPoint[]): Period[] {
  return daily.map((d) => ({
    label: d.label,
    cached: d.buckets.cached,
    cacheCreation: d.buckets.cacheCreation,
    input: d.buckets.input,
    output: d.buckets.output,
    total: d.total,
    usd: d.usd,
    turns: d.turns,
  }));
}

/** Weeks = each 7 consecutive days from the oldest; label = first day's label. */
function weekPeriods(daily: readonly DayPoint[]): Period[] {
  const out: Period[] = [];
  for (let i = 0; i < daily.length; i += 7) {
    const chunk = daily.slice(i, i + 7);
    const first = chunk[0];
    if (!first) continue;
    let cached = 0;
    let cacheCreation = 0;
    let input = 0;
    let output = 0;
    let total = 0;
    let turns = 0;
    let usdSum = 0;
    let usdAny = false;
    for (const d of chunk) {
      cached += d.buckets.cached;
      cacheCreation += d.buckets.cacheCreation;
      input += d.buckets.input;
      output += d.buckets.output;
      total += d.total;
      turns += d.turns;
      if (d.usd != null) {
        usdSum += d.usd;
        usdAny = true;
      }
    }
    out.push({ label: first.label, cached, cacheCreation, input, output, total, turns, usd: usdAny ? usdSum : null });
  }
  return out;
}

const px = (n: number): string => `${n.toFixed(2)}px`;

function TipRow({ k, name, value }: { k: "cached" | "write" | "fresh" | "output"; name: string; value: string }) {
  return (
    <span className="uv2-trend__row">
      <i className={`uv2-key uv2-trend__k-${k}`} />
      {name}
      <em className="num">{value}</em>
    </span>
  );
}

export function UsageTrend({ ins }: { ins: UsageInsights }) {
  const t = useT();
  const [grain, setGrain] = useState<Grain>(ins.daily.length <= 31 ? "day" : "week");
  const [hot, setHot] = useState<number | null>(null);
  const chartRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);

  // The cost overlay is drawn in real pixels, so track the chart's width.
  useEffect(() => {
    const el = chartRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (box) setWidth(box.width);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  if (!ins.daily.some((d) => d.total > 0)) {
    return (
      <section className="uv2-sec uv2-trend">
        <SectionHead title={t("usageV2.trend.title")} meta={t("usageV2.trend.meta")} />
        <p className="uv2-trend__empty">{t("usageV2.trend.empty")}</p>
      </section>
    );
  }

  const periods = grain === "day" ? dayPeriods(ins.daily) : weekPeriods(ins.daily);
  const n = periods.length;
  const max = Math.max(...periods.map((p) => p.total), 1);
  // X labels under every Nth column so at most ~7 show.
  const step = Math.max(1, Math.ceil(n / 7));

  let peak: Period | undefined;
  for (const p of periods) if (!peak || p.total > peak.total) peak = p;

  // Dashed cost line: y scaled to the max period usd, x through column centres.
  let costLine = "";
  if (width > 0 && periods.some((p) => p.usd != null)) {
    const cw = (width - (n - 1) * GAP) / n;
    const maxUsd = Math.max(...periods.map((p) => p.usd ?? 0), 1e-9);
    costLine = periods
      .flatMap((p, i) => {
        if (p.usd == null) return [];
        const x = i * (cw + GAP) + cw / 2;
        const y = Math.max(3, H - 2 - (p.usd / maxUsd) * (H - 6));
        return [`${x.toFixed(1)},${y.toFixed(1)}`];
      })
      .join(" ");
  }

  return (
    <section className="uv2-sec uv2-trend">
      <SectionHead
        title={t("usageV2.trend.title")}
        meta={t("usageV2.trend.meta")}
        right={
          <Seg<Grain>
            label={t("usageV2.trend.grain")}
            value={grain}
            options={[
              { value: "day", label: t("usageV2.trend.day") },
              { value: "week", label: t("usageV2.trend.week") },
            ]}
            onChange={(g) => {
              setGrain(g);
              setHot(null);
            }}
          />
        }
      />
      <div ref={chartRef} className="uv2-trend__chart" role="img" aria-label={t("usageV2.trend.chart", { n })}>
        {periods.map((p, i) => (
          <button
            key={`${p.label}-${i}`}
            type="button"
            className={i < n / 2 ? "uv2-trend__col uv2-trend__col--lh" : "uv2-trend__col uv2-trend__col--rh"}
            aria-label={t("usageV2.trend.col", {
              date: p.label,
              tokens: fmtTokens(p.total),
              usd: fmtUsd(p.usd),
              turns: p.turns.toLocaleString(),
            })}
            onMouseEnter={() => setHot(i)}
            onFocus={() => setHot(i)}
            onMouseLeave={() => setHot((h) => (h === i ? null : h))}
            onBlur={() => setHot((h) => (h === i ? null : h))}
          >
            {hot === i && (
              <span className="uv2-trend__tip">
                <b>{p.label}</b>
                <TipRow k="cached" name={t("usageV2.trend.cached")} value={fmtTokens(p.cached)} />
                <TipRow k="write" name={t("usageV2.trend.cacheWrite")} value={fmtTokens(p.cacheCreation)} />
                <TipRow k="fresh" name={t("usageV2.trend.fresh")} value={fmtTokens(p.input)} />
                <TipRow k="output" name={t("usageV2.trend.output")} value={fmtTokens(p.output)} />
                <span className="uv2-trend__tipfoot">
                  {t("usageV2.trend.tipFoot", { usd: fmtUsd(p.usd), turns: p.turns.toLocaleString() })}
                </span>
              </span>
            )}
            <span className="uv2-trend__stack">
              <i
                className="uv2-trend__seg uv2-trend__seg--top uv2-trend__k-output"
                style={{ height: px((p.output / max) * H) }}
              />
              <i className="uv2-trend__seg uv2-trend__k-fresh" style={{ height: px((p.input / max) * H) }} />
              <i className="uv2-trend__seg uv2-trend__k-write" style={{ height: px((p.cacheCreation / max) * H) }} />
              <i className="uv2-trend__seg uv2-trend__k-cached" style={{ height: px((p.cached / max) * H) }} />
            </span>
            {i % step === 0 && <span className="uv2-trend__x">{p.label}</span>}
          </button>
        ))}
        {costLine && (
          <svg className="uv2-trend__cost" width={width} height={H} viewBox={`0 0 ${width} ${H}`} aria-hidden="true">
            <polyline
              points={costLine}
              fill="none"
              stroke="var(--uv2-cost)"
              strokeWidth={1.25}
              strokeDasharray="3 3"
              strokeLinejoin="round"
              strokeLinecap="round"
            />
          </svg>
        )}
      </div>
      <div className="uv2-trend__legend">
        <span>
          <i className="uv2-key uv2-trend__k-cached" />
          {t("usageV2.trend.cached")}
        </span>
        <span>
          <i className="uv2-key uv2-trend__k-write" />
          {t("usageV2.trend.cacheWrite")}
        </span>
        <span>
          <i className="uv2-key uv2-trend__k-fresh" />
          {t("usageV2.trend.fresh")}
        </span>
        <span>
          <i className="uv2-key uv2-trend__k-output" />
          {t("usageV2.trend.output")}
        </span>
        {peak && (
          <span className="uv2-trend__peak">
            {t("usageV2.trend.peak", { label: peak.label, tokens: fmtTokens(peak.total) })}
          </span>
        )}
      </div>
    </section>
  );
}
