/**
 * UsageHero — the five-figure summary band of the Token usage page v2 (task A):
 * totals, list-price cost, cache hit, turns and active time, one column each.
 */
import type { ReactNode } from "react";
import { useT } from "../../lib/i18n";
import type { UsageInsights } from "../../lib/usageInsights";
import { SourceTag, Sparkline, fmtDelta, fmtMs, fmtPct, fmtTokens, fmtUsd } from "./primitives";

function HeroCol({
  values,
  color,
  value,
  noun,
  detail,
  detailUp = false,
  tag,
}: {
  values: readonly number[];
  color?: string;
  value: string;
  noun: string;
  detail?: ReactNode;
  detailUp?: boolean;
  tag: "measured" | "estimated" | "derived" | "priced" | "clock";
}) {
  return (
    <div className="uv2-hero__col">
      <Sparkline values={values} color={color} />
      <div className="uv2-hero__num">{value}</div>
      <div className="uv2-hero__noun">{noun}</div>
      <div className={"uv2-hero__det" + (detailUp ? " is-up" : "")}>{detail}</div>
      <SourceTag source={tag} />
    </div>
  );
}

export function UsageHero({ ins }: { ins: UsageInsights }) {
  const t = useT();

  if (ins.totals.total === 0) {
    return (
      <section className="uv2-hero uv2-hero--empty">
        <p className="uv2-hero__empty">{t("usageV2.hero.empty")}</p>
      </section>
    );
  }

  const tps = ins.tps?.p50 ?? null;
  const ttft = ins.ttft?.p50 ?? null;
  let timeDetail: string | null = null;
  if (tps != null && ttft != null) {
    timeDetail = t("usageV2.hero.time.both", { tps: Math.round(tps), ttft: fmtMs(ttft) });
  } else if (tps != null) {
    timeDetail = t("usageV2.hero.time.tps", { tps: Math.round(tps) });
  } else if (ttft != null) {
    timeDetail = t("usageV2.hero.time.ttft", { ttft: fmtMs(ttft) });
  }

  const delta = ins.delta.total;

  return (
    <section className="uv2-hero">
      <HeroCol
        values={ins.daily.map((d) => d.total)}
        value={fmtTokens(ins.totals.total)}
        noun={t("usageV2.hero.total.noun")}
        detail={delta == null ? undefined : t("usageV2.hero.total.delta", { delta: fmtDelta(delta) })}
        detailUp={delta != null && delta >= 0}
        tag="measured"
      />
      <HeroCol
        values={ins.daily.map((d) => d.usd ?? 0)}
        color="var(--uv2-cost)"
        value={ins.usd.value == null ? "—" : `≈ ${fmtUsd(ins.usd.value)}`}
        noun={t("usageV2.hero.cost.noun")}
        detail={
          <>
            {ins.usd.value == null
              ? t("usageV2.hero.cost.unpriced")
              : t("usageV2.hero.cost.covered", { pct: fmtPct(ins.usd.coverage, 0) })}
            {ins.usd.reported > 0 && (
              <span className="uv2-hero__det-seg">{t("usageV2.hero.cost.reported", { usd: fmtUsd(ins.usd.reported) })}</span>
            )}
            {ins.usd.peakExtra > 0 && (
              <span className="uv2-hero__det-seg">{t("usageV2.hero.cost.peak", { usd: fmtUsd(ins.usd.peakExtra) })}</span>
            )}
          </>
        }
        tag="priced"
      />
      <HeroCol
        values={ins.daily.map((d) => {
          // Per-day cache hit from the buckets themselves (0 when nothing was read).
          const b = d.buckets;
          const denom = b.input + b.cached + b.cacheCreation;
          return denom > 0 ? b.cached / denom : 0;
        })}
        value={fmtPct(ins.hit)}
        noun={t("usageV2.hero.hit.noun")}
        detail={t("usageV2.hero.hit.formula")}
        tag="measured"
      />
      <HeroCol
        values={ins.daily.map((d) => d.turns)}
        color="var(--text-strong)"
        value={ins.totals.turns.toLocaleString()}
        noun={t("usageV2.hero.turns.noun", { sessions: ins.totals.sessions })}
        detail={t("usageV2.hero.turns.detail", { agents: ins.totals.agents, models: ins.totals.models })}
        tag="measured"
      />
      <HeroCol
        values={ins.timingDaily.map((d) => d.ttft + d.think + d.answer + d.tool + d.other)}
        color="var(--agent-ink, var(--accent))"
        value={ins.totals.wallMs > 0 ? fmtMs(ins.totals.wallMs) : "—"}
        noun={t("usageV2.hero.time.noun")}
        detail={timeDetail ?? undefined}
        tag="clock"
      />
    </section>
  );
}
