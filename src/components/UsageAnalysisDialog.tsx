import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { X } from "lucide-react";
import { useT } from "../lib/i18n";
import { formatTokenCount } from "../lib/sessionStats";
import {
  aggregateDays,
  dailySeries,
  deltaPercent,
  rankedModels,
  useUsageLedger,
  type LedgerAggregate,
} from "../lib/usageLedger";
import {
  costOfBuckets,
  fetchPriceCatalog,
  formatUsd,
  rateForModel,
  type RateIndex,
} from "../lib/tokenPrices";
import { UsageHeatmap } from "./UsageHeatmap";

/**
 * Token 用量分析大面板——入口只有两个:右栏「Token 总览」卡的链接与
 * Ctrl K 的「Token 用量分析」(App 经 openUsageAnalysis 挂进来),主界面
 * 不加按钮。版式走 project-dialog 骨架 + ui-language 的卡/芯片/身份色。
 *
 * 内容(2026-11-01 定案,产品口径):总计+环比 → 四桶(读取/输出/缓存读/
 * 缓存写) → GitHub 式每日格子 + 逐日堆叠柱 → 模型环形+排名(>7 折叠
 * 「其它」) → 费用(models.dev 牌价折算,拉不到整段隐藏;明确标注
 * 「非订阅实付」)。图全部零依赖手绘(SpecCard 同路线)。
 */

const GRID_WEEKS = 26;
const DONUT_R = 48;
const DONUT_C = 2 * Math.PI * DONUT_R;

function pct(n: number): string {
  return `${Math.round(n * 100)}%`;
}

function signedPct(n: number): string {
  const s = Math.round(Math.abs(n) * 100);
  return `${n > 0 ? "+" : n < 0 ? "−" : ""}${s}%`;
}

function dayTotal(day: { input: number; output: number; cached: number; cacheCreation: number }) {
  return day.input + day.output + day.cached + day.cacheCreation;
}

function aggregateTotal(agg: LedgerAggregate): number {
  return agg.input + agg.output + agg.cached + agg.cacheCreation;
}

export function UsageAnalysisDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  const ledger = useUsageLedger();
  const [rangeDays, setRangeDays] = useState<7 | 30>(7);
  const [priceIndex, setPriceIndex] = useState<RateIndex | null>(null);
  // ⚠ 本组件所有 hooks 必须在任何条件 return 之前(见 MarkdownBody 红线)。
  const now = useMemo(() => new Date(), []);
  const hasAnyRecords = useMemo(
    () => aggregateDays(ledger, 180, now).turns > 0,
    [ledger, now],
  );

  useEffect(() => {
    let alive = true;
    void fetchPriceCatalog().then((index) => {
      if (alive) setPriceIndex(index);
    });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const prevDays = useMemo(() => {
    const prev = new Date(now);
    prev.setDate(prev.getDate() - rangeDays);
    return prev;
  }, [now, rangeDays]);

  const current = useMemo(() => aggregateDays(ledger, rangeDays, now), [ledger, rangeDays, now]);
  const previous = useMemo(() => aggregateDays(ledger, rangeDays, prevDays), [ledger, rangeDays, prevDays]);
  const series = useMemo(() => dailySeries(ledger, rangeDays, now), [ledger, rangeDays, now]);
  const gridSeries = useMemo(() => dailySeries(ledger, GRID_WEEKS * 7, now), [ledger, now]);
  const models = useMemo(() => rankedModels(current, 6), [current]);

  const maxDay = useMemo(
    () => Math.max(1, ...series.map(dayTotal)),
    [series],
  );

  const delta = useMemo(
    () => deltaPercent(aggregateTotal(current), aggregateTotal(previous)),
    [current, previous],
  );

  const cacheHit = useMemo(() => {
    const denom = current.input + current.cached;
    return denom > 0 ? current.cached / denom : null;
  }, [current]);

  /** 费用只在牌价目录可用时出现;未计价 token 计入覆盖率分母(codeg 同口径)。 */
  const costs = useMemo(() => {
    if (!priceIndex || priceIndex.byModel.size === 0) return null;
    let usd = 0;
    let priced = 0;
    let unpriced = 0;
    for (const row of models) {
      const cost = costOfBuckets(row, rateForModel(row.modelId, priceIndex));
      if (cost.usd != null) usd += cost.usd;
      priced += cost.pricedTokens;
      unpriced += cost.unpricedTokens;
    }
    if (priced + unpriced === 0) return null;
    return { usd, coverage: priced / (priced + unpriced), fetchedAt: priceIndex.fetchedAt };
  }, [models, priceIndex]);

  const donut = useMemo(() => {
    const rows = models
      .map((row, i) => ({ ...row, share: dayTotal(row), colorClass: `um-${row.modelId === "other" ? "other" : Math.min(i, 5)}` }))
      .filter((row) => row.share > 0);
    const total = rows.reduce((sum, row) => sum + row.share, 0);
    if (total <= 0) return null;
    let offset = 0;
    const segments = rows.map((row) => {
      const frac = row.share / total;
      const seg = { ...row, frac, offset };
      offset += frac;
      return seg;
    });
    return { segments, top: segments[0]!, total };
  }, [models]);

  const rangeLabel = rangeDays === 7 ? t("usageLedger.last7") : t("usageLedger.last30");

  return (
    <div className="project-dialog-backdrop" onClick={onClose}>
      <div
        className="project-dialog usage-analysis-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("usageLedger.analysisTitle")}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="project-dialog__header">
          <span>{t("usageLedger.analysisTitle")}</span>
          <button
            type="button"
            className="pill-action pill-action--icon"
            aria-label={t("common.close")}
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </div>
        <div className="project-dialog__body usage-analysis">
          {!hasAnyRecords ? (
            <div className="usage-analysis__empty usage-analysis__card">
              <p>{t("usageLedger.emptyTitle")}</p>
              <p>{t("usageLedger.emptyNote")}</p>
            </div>
          ) : (
            <>
              <div className="usage-analysis__toolbar">
                <div className="usage-analysis__ranges" role="tablist">
                  {([7, 30] as const).map((days) => (
                    <button
                      key={days}
                      type="button"
                      role="tab"
                      aria-selected={rangeDays === days}
                      className={`composer-mode-chip${rangeDays === days ? " is-active" : ""}`}
                      onClick={() => setRangeDays(days)}
                    >
                      {days === 7 ? t("usageLedger.last7") : t("usageLedger.last30")}
                    </button>
                  ))}
                </div>
                {costs?.fetchedAt != null && (
                  <span className="usage-analysis__sub">
                    {t("usageLedger.priceUpdated", {
                      when: new Date(costs.fetchedAt).toLocaleDateString(),
                    })}
                  </span>
                )}
              </div>

              <section className="usage-analysis__card">
                <div className="usage-analysis__hero">
                  <span className="usage-analysis__total">
                    {formatTokenCount(aggregateTotal(current))}
                  </span>
                  {delta != null && (
                    <span
                      className="usage-analysis__delta"
                      data-dir={delta > 0.005 ? "up" : delta < -0.005 ? "down" : "flat"}
                    >
                      {signedPct(delta)}
                    </span>
                  )}
                  <span className="usage-analysis__sub">{rangeLabel}</span>
                </div>
                <div className="usage-analysis__buckets">
                  <div className="usage-analysis__bucket" style={{ "--bucket": "var(--blue)" } as CSSProperties}>
                    <div className="usage-analysis__bucket-label">{t("usageLedger.bucketInput")}</div>
                    <div className="usage-analysis__bucket-value">{formatTokenCount(current.input)}</div>
                  </div>
                  <div className="usage-analysis__bucket" style={{ "--bucket": "var(--accent)" } as CSSProperties}>
                    <div className="usage-analysis__bucket-label">{t("usageLedger.bucketOutput")}</div>
                    <div className="usage-analysis__bucket-value">{formatTokenCount(current.output)}</div>
                  </div>
                  <div className="usage-analysis__bucket" style={{ "--bucket": "var(--green)" } as CSSProperties}>
                    <div className="usage-analysis__bucket-label">{t("usageLedger.bucketCacheRead")}</div>
                    <div className="usage-analysis__bucket-value">{formatTokenCount(current.cached)}</div>
                  </div>
                  <div className="usage-analysis__bucket" style={{ "--bucket": "var(--orange)" } as CSSProperties}>
                    <div className="usage-analysis__bucket-label">{t("usageLedger.bucketCacheWrite")}</div>
                    <div className="usage-analysis__bucket-value">{formatTokenCount(current.cacheCreation)}</div>
                  </div>
                </div>
                <div className="usage-analysis__sub" style={{ marginTop: 6 }}>
                  {t("usageLedger.turns", { count: current.turns })}
                  {cacheHit != null && ` · ${t("usageLedger.cacheHit", { pct: pct(cacheHit) })}`}
                </div>
              </section>

              <section className="usage-analysis__card">
                <div className="usage-analysis__kicker">{t("usageLedger.dailyTitle")}</div>
                <UsageHeatmap
                  days={gridSeries.map((day) => ({
                    key: day.key,
                    value: dayTotal(day),
                    turns: day.turns,
                  }))}
                />
                <div className="usage-analysis__bars" aria-hidden>
                  {series.map((day) => {
                    const total = dayTotal(day);
                    const scale = total > 0 ? Math.max(0.04, total / maxDay) : 0;
                    const segs: Array<{ cls: string; value: number }> = [
                      { cls: "out", value: day.output },
                      { cls: "in", value: day.input },
                      { cls: "cr", value: day.cached },
                      { cls: "cw", value: day.cacheCreation },
                    ];
                    return (
                      <div key={day.key} className="usage-analysis__bar" title={`${day.key} · ${formatTokenCount(total)}`}>
                        {total > 0 &&
                          segs.map((seg) =>
                            seg.value > 0 ? (
                              <div
                                key={seg.cls}
                                className={`usage-analysis__bar-seg usage-analysis__bar-seg--${seg.cls}`}
                                style={{ height: `${(seg.value / total) * scale * 100}%` }}
                              />
                            ) : null,
                          )}
                      </div>
                    );
                  })}
                </div>
                <div className="usage-analysis__bar-labels" aria-hidden>
                  <span>{series[0]?.key.slice(5)}</span>
                  <span>{rangeLabel}</span>
                  <span>{series[series.length - 1]?.key.slice(5)}</span>
                </div>
                <div className="usage-analysis__legend">
                  <span><i style={{ background: "var(--blue)" }} />{t("usageLedger.bucketInput")}</span>
                  <span><i style={{ background: "var(--accent)" }} />{t("usageLedger.bucketOutput")}</span>
                  <span><i style={{ background: "var(--green)" }} />{t("usageLedger.bucketCacheRead")}</span>
                  <span><i style={{ background: "var(--orange)" }} />{t("usageLedger.bucketCacheWrite")}</span>
                </div>
              </section>

              <section className="usage-analysis__card">
                <div className="usage-analysis__kicker">{t("usageLedger.modelsTitle")}</div>
                <div className="usage-analysis__models">
                  {donut && (
                    <div className="usage-analysis__donut">
                      <svg viewBox="0 0 120 120" width="120" height="120" aria-hidden>
                        {donut.segments.map((seg) => {
                          const colorVar = `var(--${["accent", "blue", "orange", "green", "yellow", "red"][Math.min(donut.segments.indexOf(seg), 5)]}, var(--text-faint))`;
                          return (
                            <circle
                              key={seg.modelId}
                              cx="60"
                              cy="60"
                              r={DONUT_R}
                              fill="none"
                              stroke={seg.modelId === "other" ? "var(--text-faint)" : colorVar}
                              strokeWidth="14"
                              strokeDasharray={`${seg.frac * DONUT_C} ${DONUT_C}`}
                              strokeDashoffset={-seg.offset * DONUT_C}
                              transform="rotate(-90 60 60)"
                            />
                          );
                        })}
                      </svg>
                      <div className="usage-analysis__donut-center">
                        <span className="usage-analysis__donut-pct">{pct(donut.top.frac)}</span>
                        <span className="usage-analysis__donut-name">
                          {donut.top.modelId === "other" ? t("usageLedger.other") : donut.top.modelId}
                        </span>
                      </div>
                    </div>
                  )}
                  <div className="usage-analysis__rows">
                    {models.map((row) => {
                      const cost =
                        costs && priceIndex
                          ? costOfBuckets(row, rateForModel(row.modelId, priceIndex))
                          : null;
                      return (
                        <div key={row.modelId} className={`usage-analysis__row ${row.modelId === "other" ? "um-other" : `um-${Math.min(models.indexOf(row), 5)}`}`}>
                          <span className="usage-analysis__row-dot" aria-hidden />
                          <span className="usage-analysis__row-name">
                            {row.modelId === "other" ? t("usageLedger.other") : row.modelId}
                          </span>
                          <span className="usage-analysis__row-meta">
                            {formatTokenCount(dayTotal(row))} · {t("usageLedger.turns", { count: row.turns })}
                          </span>
                          {cost?.usd != null && cost.usd > 0 && (
                            <span className="usage-analysis__row-cost">{formatUsd(cost.usd)}</span>
                          )}
                        </div>
                      );
                    })}
                  </div>
                </div>
              </section>

              {costs && costs.usd > 0 && (
                <section className="usage-analysis__card">
                  <div className="usage-analysis__cost">
                    <span className="usage-analysis__cost-total">{formatUsd(costs.usd)}</span>
                    <span className="usage-analysis__sub">
                      {t("usageLedger.coverage", { pct: pct(costs.coverage) })}
                    </span>
                  </div>
                  <p className="usage-analysis__cost-note">{t("usageLedger.costNote")}</p>
                </section>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
