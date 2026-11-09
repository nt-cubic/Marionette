/**
 * Token usage page v2 · rhythm (task D): the 7 × 24 hour-of-day × weekday
 * turn grid ("作息") and the end-of-turn context occupancy histogram with its
 * three summary stats ("上下文"). Data comes from usageInsights.
 */
import type { CSSProperties } from "react";
import { useT, type DictKey } from "../../lib/i18n";
import { SectionHead } from "./primitives";
import type { UsageInsights } from "../../lib/usageInsights";

const DAYS = 7;
const HOURS = 24;

/** 7-step ramp index on a square-root scale: 0 = empty, 1..6 = accent mix. */
function rampStep(v: number, max: number): number {
  if (v <= 0 || max <= 0) return 0;
  return Math.min(6, 1 + Math.floor(Math.sqrt(v / max) * 6));
}

/** Smallest bucket whose cumulative count reaches the median; null when empty. */
function medianBucket(hist: readonly number[]): number | null {
  const total = hist.reduce((a, b) => a + b, 0);
  if (total <= 0) return null;
  let seen = 0;
  for (let i = 0; i < hist.length; i++) {
    seen += hist[i] ?? 0;
    if (seen * 2 >= total) return i;
  }
  return null;
}

function HourGrid({ grid }: { grid: readonly number[][] }) {
  const t = useT();
  const max = Math.max(0, ...grid.flat());
  return (
    <div className="uv2-rhythm__hm" role="img" aria-label={t("usageV2.rhythm.gridAria")}>
      {Array.from({ length: DAYS }, (_, d) => (
        <div className="uv2-rhythm__hmrow" key={d}>
          <span>{t(`usageV2.rhythm.dow.${d}` as DictKey)}</span>
          {Array.from({ length: HOURS }, (_, h) => {
            const v = grid[d]?.[h] ?? 0;
            const step = rampStep(v, max);
            return (
              <i
                key={h}
                className={step > 0 ? `uv2-rhythm__cell s${step}` : "uv2-rhythm__cell"}
                title={t("usageV2.rhythm.cellTitle", {
                  day: t(`usageV2.rhythm.dowFull.${d}` as DictKey),
                  hour: `${String(h).padStart(2, "0")}:00`,
                  n: v,
                })}
              />
            );
          })}
        </div>
      ))}
      <div className="uv2-rhythm__hmaxis" aria-hidden="true">
        <span />
        {Array.from({ length: HOURS }, (_, h) => (
          <span key={h}>{h % 3 === 0 ? h : ""}</span>
        ))}
      </div>
    </div>
  );
}

function CtxChart({ hist, hot }: { hist: readonly number[]; hot: number }) {
  const t = useT();
  const total = hist.reduce((a, b) => a + b, 0);
  const max = Math.max(0, ...hist);
  const median = medianBucket(hist);
  if (total <= 0) return <p className="uv2-rhythm__empty">{t("usageV2.rhythm.ctxNone")}</p>;
  return (
    <>
      <div className="uv2-rhythm__pkw" role="img" aria-label={t("usageV2.rhythm.chartAria")}>
        {hist.map((v, i) => (
          <div className={i >= 8 ? "uv2-rhythm__pk is-hot" : "uv2-rhythm__pk"} key={i}>
            <i style={{ height: `${v > 0 ? Math.max(6, (v / max) * 100) : 0}%` }} />
            <span>{i * 10}</span>
          </div>
        ))}
      </div>
      <div className="uv2-rhythm__ev">
        <div>
          <b>{hot}</b>
          <span>{t("usageV2.rhythm.ctxHot")}</span>
        </div>
        <div>
          <b>{total.toLocaleString()}</b>
          <span>{t("usageV2.rhythm.ctxReads")}</span>
        </div>
        {median != null && (
          <div>
            <b>{t("usageV2.rhythm.medianRange", { lo: median * 10, hi: median * 10 + 10 })}</b>
            <span>{t("usageV2.rhythm.ctxMedian")}</span>
          </div>
        )}
      </div>
    </>
  );
}

/** Turn outcomes under the context chart: compactions, failed and cancelled turns. */
function OutcomeCells({ ins }: { ins: UsageInsights }) {
  const t = useT();
  const cells: Array<[number, string]> = [
    [ins.compactions, t("usageV2.rhythm.compactions")],
    [ins.turnFails, t("usageV2.rhythm.fails")],
    [ins.turnCancels, t("usageV2.rhythm.cancels")],
  ];
  return (
    <div className="uv2-rhythm__ev uv2-rhythm__ev--outcomes">
      {cells.map(([n, label]) => (
        <div key={label}>
          <b>{n.toLocaleString()}</b>
          <span>{label}</span>
        </div>
      ))}
    </div>
  );
}

export function UsageRhythm({ ins }: { ins: UsageInsights }) {
  const t = useT();
  return (
    <div className="uv2-cols uv2-rhythm" style={{ "--uv2-cols": 2 } as CSSProperties}>
      <section className="uv2-sec">
        <SectionHead title={t("usageV2.rhythm.hours")} meta={t("usageV2.rhythm.hoursMeta")} />
        <HourGrid grid={ins.hourGrid} />
      </section>
      <section className="uv2-sec">
        <SectionHead title={t("usageV2.rhythm.ctx")} meta={t("usageV2.rhythm.ctxMeta")} />
        <CtxChart hist={ins.ctxHist} hot={ins.ctxHot} />
        <OutcomeCells ins={ins} />
      </section>
    </div>
  );
}
