/**
 * UsageHero - first-screen summary band of the Token usage page (T3 hierarchy).
 *
 * It answers the four questions the page must answer without scrolling: the
 * period's headline figure (cost or tokens), how many sessions/turns that came
 * from, how many days were active, the cache-hit share, and - on the Cost view -
 * how much of the token volume could actually be priced. The detailed charts
 * live behind the expandable layer, so this band stays small.
 */
import { useT } from "../../lib/i18n";
import type { UsageInsights } from "../../lib/usageInsights";
import { SourceTag, Sparkline, fmtDelta, fmtPct, fmtTokens, fmtUsd } from "./primitives";

export function UsageHero({
  ins,
  view,
  customPrices = false,
}: {
  ins: UsageInsights;
  view: "cost" | "tokens";
  customPrices?: boolean;
}) {
  const t = useT();
  const costLead = view === "cost";

  const leadValue = costLead
    ? ins.usd.value == null
      ? "\u2014"
      : `\u2248 ${fmtUsd(ins.usd.value)}`
    : fmtTokens(ins.totals.total);
  const leadNoun = costLead ? t("usageV2.hero.cost.noun") : t("usageV2.hero.total.noun");
  const leadSeries = ins.daily.map((day) => (costLead ? (day.usd ?? 0) : day.total));
  const delta = costLead ? ins.delta.usd : ins.delta.total;

  const secondaryValue = costLead
    ? fmtTokens(ins.totals.total)
    : ins.usd.value == null
      ? "\u2014"
      : `\u2248 ${fmtUsd(ins.usd.value)}`;
  const secondaryLabel = costLead ? t("usageV2.hero.tokensCell") : t("usageV2.hero.costCell");

  const cells = [
    { label: t("usageV2.hero.session"), value: ins.totals.sessions.toLocaleString() },
    { label: t("usageV2.hero.turnsCell"), value: ins.totals.turns.toLocaleString() },
    { label: t("usageV2.hero.activeDays"), value: ins.totals.activeDays.toLocaleString() },
    { label: t("usageV2.hero.hit.noun"), value: fmtPct(ins.hit) },
    { label: secondaryLabel, value: secondaryValue },
  ];

  const meta = costLead
    ? ins.usd.value == null
      ? t("usageV2.hero.cost.unpriced")
      : t("usageV2.hero.cost.covered", { pct: fmtPct(ins.usd.coverage, 0) })
    : delta == null
      ? t("usageV2.hero.turns.detail", { agents: ins.totals.agents, models: ins.totals.models })
      : t("usageV2.hero.total.delta", { delta: fmtDelta(delta) });

  return (
    <section className="uv2-hero">
      <div className="uv2-hero__lead">
        <Sparkline
          values={leadSeries}
          color={costLead ? "var(--uv2-cost)" : "var(--accent)"}
          width={200}
          height={34}
        />
        <div className="uv2-hero__num">{leadValue}</div>
        <div className="uv2-hero__noun">{leadNoun}</div>
        <div className="uv2-hero__meta">
          {meta}
          <SourceTag source={costLead ? (customPrices ? "custom" : "priced") : "measured"} />
        </div>
        {costLead && (delta != null || ins.usd.reported > 0 || ins.usd.peakExtra > 0) && (
          <div className="uv2-hero__sub">
            {delta != null && <span>{t("usageV2.hero.total.delta", { delta: fmtDelta(delta) })}</span>}
            {ins.usd.reported > 0 && (
              <span>{t("usageV2.hero.cost.reported", { usd: fmtUsd(ins.usd.reported) })}</span>
            )}
            {ins.usd.peakExtra > 0 && (
              <span>{t("usageV2.hero.cost.peak", { usd: fmtUsd(ins.usd.peakExtra) })}</span>
            )}
          </div>
        )}
      </div>
      <div className="uv2-hero__stats">
        {cells.map((cell) => (
          <div className="uv2-hero__cell" key={cell.label}>
            <b>{cell.value}</b>
            <span>{cell.label}</span>
          </div>
        ))}
      </div>
    </section>
  );
}
