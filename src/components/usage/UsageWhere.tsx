/**
 * UsageWhere — the「Token 去向 / 时间去向」band pair of the Token usage page v2
 * (canvas board「v2 · Token 用量」). Left: the token composition treemap plus its
 * provenance rows; right: the daily time ribbon, the overall split stack, timing
 * rows and latency / throughput percentiles.
 */
import type { CSSProperties } from "react";
import { useT } from "../../lib/i18n";
import type { CompositionKey, CompositionPart, TimingKey, UsageInsights } from "../../lib/usageInsights";
import { fmtMs, fmtPct, fmtTokens, SectionHead, SourceTag } from "./primitives";

const COMP_ORDER: readonly CompositionKey[] = ["cached", "toolIo", "agentInternal", "user", "thinking", "answer"];
const TIME_ORDER: readonly TimingKey[] = ["ttft", "think", "answer", "tool", "other"];

/** Block / key colours per composition key; `ink` marks the light-text blocks. */
const COMP_STYLE: Record<CompositionKey, { color: string; ink: boolean }> = {
  cached: { color: "var(--uv2-cache-read)", ink: false },
  toolIo: { color: "var(--uv2-tool)", ink: false },
  agentInternal: { color: "var(--uv2-derived)", ink: true },
  user: { color: "var(--panel-soft)", ink: false },
  thinking: { color: "var(--uv2-think)", ink: true },
  answer: { color: "var(--uv2-output)", ink: true },
};

const TIME_COLOR: Record<TimingKey, string> = {
  ttft: "var(--uv2-wait)",
  think: "var(--uv2-think)",
  answer: "var(--uv2-output)",
  tool: "var(--uv2-tool)",
  other: "var(--uv2-other)",
};

/** One treemap block: name + "pct · tokens" bottom-left, SourceTag top-right. */
function CompBlock({ part, flex, small, full }: { part: CompositionPart; flex?: number; small?: boolean; full?: boolean }) {
  const t = useT();
  const style = COMP_STYLE[part.key];
  const cls = `uv2-where-blk${style.ink ? " is-ink" : ""}${small ? " is-sm" : ""}`;
  const css: CSSProperties = { background: style.color };
  if (flex != null) {
    css.flexGrow = flex;
    css.flexBasis = 0;
  }
  if (full) css.gridColumn = "1 / -1";
  return (
    // Slivers under 3% carry colour only (the list below names them); a title keeps them readable on hover.
    <div
      className={cls}
      style={css}
      title={`${t(`usageV2.where.c.${part.key}.short`)} · ${fmtPct(part.share)} · ${fmtTokens(part.tokens)}`}
    >
      {part.share >= 0.04 && <SourceTag source={part.source} />}
      {part.share >= 0.03 && (
        <>
          <b>{t(`usageV2.where.c.${part.key}.short`)}</b>
          <span>{`${fmtPct(part.share)} · ${fmtTokens(part.tokens)}`}</span>
        </>
      )}
    </div>
  );
}

export function UsageWhere({ ins }: { ins: UsageInsights }) {
  const t = useT();

  // Left band: composition sorted by size; the largest part owns the left column,
  // its width clamped to 35–65% of the band regardless of its actual share.
  const comp = ins.hasDetail
    ? ins.composition.filter((p) => p.tokens > 0).sort((a, b) => b.tokens - a.tokens)
    : [];
  const compByKey = new Map(comp.map((p) => [p.key, p]));
  const head = comp[0];
  const headShare = head == null ? 0 : Math.min(0.65, Math.max(0.35, head.share));
  const rest = comp.slice(1);
  const rowsTm = rest.filter((p) => p.share >= 0.06);
  const smalls = rest.filter((p) => p.share < 0.06);

  // Right band: timing split, per-day ribbon totals and the axis labels.
  const timeByKey = new Map(ins.timing.map((p) => [p.key, p]));
  const timeTotal = ins.timing.reduce((s, p) => s + p.ms, 0);
  const dailyTotals = ins.timingDaily.map((day) => TIME_ORDER.reduce((s, k) => s + day[k], 0));
  const maxDaily = Math.max(1, ...dailyTotals);
  const firstLabel = ins.daily[0]?.label;
  const lastLabel = ins.daily.length > 1 ? ins.daily[ins.daily.length - 1]?.label : undefined;

  return (
    <div className="uv2-cols uv2-where" style={{ "--uv2-cols": 2 } as CSSProperties}>
      <section className="uv2-sec">
        <SectionHead title={t("usageV2.where.tokensTitle")} meta={t("usageV2.where.tokensMeta")} />
        {head == null ? (
          <p className="uv2-where-empty faint">{t("usageV2.noDetail")}</p>
        ) : (
          <>
            <div
              className="uv2-where-tm"
              role="img"
              aria-label={t("usageV2.where.tmAria")}
              style={{ gridTemplateColumns: `${headShare}fr ${1 - headShare}fr` }}
            >
              <CompBlock part={head} full={rest.length === 0} />
              <div className="uv2-where-tmb">
                {rowsTm.map((p) => (
                  <CompBlock key={p.key} part={p} flex={p.tokens} />
                ))}
                {smalls.length > 0 && (
                  <div className="uv2-where-tmrow">
                    {smalls.map((p) => (
                      <CompBlock key={p.key} part={p} flex={p.tokens} small />
                    ))}
                  </div>
                )}
              </div>
            </div>
            <div className="uv2-where-rows">
              {COMP_ORDER.filter((k) => compByKey.has(k)).map((k) => {
                const p = compByKey.get(k);
                return p == null ? null : (
                  <div key={k} className="uv2-where-row">
                    <i className="uv2-key" style={{ background: COMP_STYLE[k].color }} aria-hidden="true" />
                    <span className="uv2-where-name">{t(`usageV2.where.c.${k}.name`)}</span>
                    <SourceTag source={p.source} />
                    <span className="uv2-where-how">{t(`usageV2.where.c.${k}.how`)}</span>
                    <span className="num">{fmtPct(p.share)}</span>
                    <span className="num faint">{fmtTokens(p.tokens)}</span>
                  </div>
                );
              })}
            </div>
            <p className="uv2-where-note">{t("usageV2.where.tokensNote")}</p>
          </>
        )}
      </section>

      <section className="uv2-sec">
        <SectionHead title={t("usageV2.where.timeTitle")} meta={t("usageV2.where.timeMeta", { total: fmtMs(timeTotal) })} />
        {timeTotal <= 0 ? (
          <p className="uv2-where-empty faint">{t("usageV2.noDetail")}</p>
        ) : (
          <>
            <div className="uv2-where-daily" role="img" aria-label={t("usageV2.where.dailyAria")}>
              {ins.timingDaily.map((day, i) => {
                const total = dailyTotals[i] ?? 0;
                if (total <= 0) return <span key={i} className="uv2-where-stub" />;
                return (
                  <span key={i} className="uv2-where-day" style={{ height: `${(total / maxDaily) * 100}%` }}>
                    {TIME_ORDER.map((k) =>
                      day[k] > 0 ? <i key={k} style={{ flexGrow: day[k], background: TIME_COLOR[k] }} /> : null,
                    )}
                  </span>
                );
              })}
            </div>
            {(firstLabel != null || lastLabel != null) && (
              <div className="uv2-where-axis faint">
                <span>{firstLabel ?? ""}</span>
                {lastLabel != null && <span>{lastLabel}</span>}
              </div>
            )}
            <div className="uv2-where-stack" aria-hidden="true">
              {TIME_ORDER.map((k) => {
                const p = timeByKey.get(k);
                return p != null && p.share > 0 ? (
                  <i key={k} style={{ width: `${p.share * 100}%`, background: TIME_COLOR[k] }} />
                ) : null;
              })}
            </div>
            <div className="uv2-where-rows">
              {TIME_ORDER.map((k) => {
                const p = timeByKey.get(k);
                return p == null ? null : (
                  <div key={k} className="uv2-where-row uv2-where-row--time">
                    <i className="uv2-key" style={{ background: TIME_COLOR[k] }} aria-hidden="true" />
                    <span className="uv2-where-name">{t(`usageV2.where.t.${k}.name`)}</span>
                    <span className="uv2-where-how">{t(`usageV2.where.t.${k}.how`)}</span>
                    <span className="num">{fmtPct(p.share)}</span>
                    <span className="num faint">{fmtMs(p.ms)}</span>
                  </div>
                );
              })}
            </div>
            <div className="uv2-where-pct">
              <div>
                <b>{ins.ttft ? fmtMs(ins.ttft.p50) : "—"}</b>
                <span>{t("usageV2.where.pTtft50")}</span>
              </div>
              <div>
                <b>{ins.ttft ? fmtMs(ins.ttft.p95) : "—"}</b>
                <span>{t("usageV2.where.pTtft95")}</span>
              </div>
              <div>
                <b>{ins.tps ? t("usageV2.where.tps", { n: Math.round(ins.tps.p50) }) : "—"}</b>
                <span>{t("usageV2.where.pTps50")}</span>
              </div>
              <div>
                <b>{ins.tps ? t("usageV2.where.tps", { n: Math.round(ins.tps.p95) }) : "—"}</b>
                <span>{t("usageV2.where.pTps95")}</span>
              </div>
            </div>
          </>
        )}
      </section>
    </div>
  );
}
