import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ArrowDownRight, ArrowUpRight, Download, X } from "lucide-react";
import { useLocale, useT } from "../lib/i18n";
import { formatDurationCompact, formatTokenCount } from "../lib/sessionStats";
import { showToast, dismissToast } from "../lib/toast";
import { revealInFileManager } from "../lib/api";
import {
  aggregateDays,
  cacheHitRate,
  dailySeries,
  deltaPercent,
  exportUsageCsv,
  foldHourDays,
  freshTokens,
  mondayFirstHourGrid,
  peakHourOf,
  rankedAgents,
  rankedModels,
  totalTokens,
  trendSeries,
  useUsageLedger,
} from "../lib/usageLedger";
import {
  costOfBuckets,
  fetchPriceCatalog,
  formatUsd,
  rateForModel,
  type RateIndex,
} from "../lib/tokenPrices";
import {
  HourHeatmap,
  RankedBars,
  RingMeter,
  TrendChart,
  U_ACCENT,
  U_FAINT,
  U_INK,
  U_INK_2,
  U_INK_3,
  type RankedDatum,
  type TrendDatum,
} from "./UsageCharts";

/**
 * Token 用量分析——伪标签页 / 贴边整页 / detached 弹窗共用本体。
 *
 * 版式学 codeg token-usage-page:大数字英雄卡 + 缓存环、连通统计条、
 * 两色趋势柱、构成条 + 小时×星期作息、模型/助手排名。表面仍走
 * docs/ui-language.md(3px 角、1px 边、无阴影、芯片 22px)。
 */

const RANGES = [7, 30, 90] as const;
type RangeDays = (typeof RANGES)[number];
const CACHE_HIT_DIGITS = 1;

function pct(n: number, digits = 0): string {
  return `${(n * 100).toFixed(digits)}%`;
}

function signedPct(n: number): string {
  const s = Math.round(Math.abs(n) * 100);
  return `${n > 0 ? "+" : n < 0 ? "−" : ""}${s}%`;
}

function sliceDay(key: string): string {
  return key.slice(5);
}

function weekdayLabels(locale: string): string[] {
  const fmt = new Intl.DateTimeFormat(locale === "zh-CN" ? "zh-CN" : "en", { weekday: "short" });
  // 周一首行:1970-01-05 是周一。
  return Array.from({ length: 7 }, (_, i) => fmt.format(new Date(1970, 0, 5 + i)));
}

function Panel({
  title,
  hint,
  actions,
  children,
}: {
  title: string;
  hint?: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="usage-analysis__panel">
      <header className="usage-analysis__panel-head">
        <div className="usage-analysis__panel-copy">
          <h2 className="usage-analysis__kicker">{title}</h2>
          {hint ? <p className="usage-analysis__sub">{hint}</p> : null}
        </div>
        {actions}
      </header>
      {children}
    </section>
  );
}

function StatCell({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="usage-analysis__cell">
      <div className="usage-analysis__cell-label">{label}</div>
      <div className="usage-analysis__cell-value">{value}</div>
      {hint ? <div className="usage-analysis__cell-hint">{hint}</div> : null}
    </div>
  );
}

function DeltaBadge({
  delta,
  newLabel,
  title,
}: {
  delta: number | null;
  newLabel: string;
  title: string;
}) {
  if (delta == null) {
    return (
      <span className="usage-analysis__delta" data-dir="new" title={title}>
        {newLabel}
      </span>
    );
  }
  if (Math.abs(delta) < 0.005) return null;
  const up = delta > 0;
  const Icon = up ? ArrowUpRight : ArrowDownRight;
  return (
    <span className="usage-analysis__delta" data-dir={up ? "up" : "down"} title={title}>
      <Icon size={12} aria-hidden />
      {signedPct(delta)}
    </span>
  );
}

export function UsageAnalysisBody() {
  const t = useT();
  const locale = useLocale();
  const ledger = useUsageLedger();
  const [rangeDays, setRangeDays] = useState<RangeDays>(7);
  const [bucketTouched, setBucketTouched] = useState(false);
  const [bucket, setBucket] = useState<"day" | "week">("day");
  const [dim, setDim] = useState<"model" | "agent">("model");
  const [priceIndex, setPriceIndex] = useState<RateIndex | null>(null);
  const now = useMemo(() => new Date(), []);
  const bcp47 = locale === "zh-CN" ? "zh-CN" : "en";
  const weekdays = useMemo(() => weekdayLabels(bcp47), [bcp47]);

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

  const prevEnd = useMemo(() => {
    const prev = new Date(now);
    prev.setDate(prev.getDate() - rangeDays);
    return prev;
  }, [now, rangeDays]);

  const current = useMemo(() => aggregateDays(ledger, rangeDays, now), [ledger, rangeDays, now]);
  const previous = useMemo(
    () => aggregateDays(ledger, rangeDays, prevEnd),
    [ledger, rangeDays, prevEnd],
  );
  const daily = useMemo(() => dailySeries(ledger, rangeDays, now), [ledger, rangeDays, now]);
  const effectiveBucket = bucketTouched ? bucket : rangeDays >= 90 ? "week" : "day";
  const trend = useMemo(
    () => trendSeries(daily, effectiveBucket),
    [daily, effectiveBucket],
  );
  const models = useMemo(() => rankedModels(current, 7), [current]);
  const agents = useMemo(() => rankedAgents(current, 7), [current]);
  const hourGrid = useMemo(
    () => mondayFirstHourGrid(foldHourDays(ledger, rangeDays, now)),
    [ledger, rangeDays, now],
  );
  const busiestHour = useMemo(() => peakHourOf(hourGrid), [hourGrid]);

  const totals = totalTokens(current);
  const previousTotal = totalTokens(previous);
  const delta = deltaPercent(totals, previousTotal);
  const cache = cacheHitRate(current);
  const fresh = freshTokens(current);

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

  const trendData = useMemo<TrendDatum[]>(
    () =>
      trend.map((point) => ({
        key: point.key,
        label: sliceDay(point.key),
        tooltipLabel:
          effectiveBucket === "day"
            ? new Date(`${point.key}T00:00:00`).toLocaleDateString(bcp47, {
                month: "short",
                day: "numeric",
                weekday: "short",
              })
            : sliceDay(point.key),
        cache: point.cache,
        fresh: point.fresh,
        detail: [{ label: t("usageLedger.tileTurns"), value: String(point.turns) }],
      })),
    [trend, effectiveBucket, bcp47, t],
  );

  const compositionRows = useMemo<RankedDatum[]>(() => {
    if (totals <= 0) return [];
    const rows = [
      { key: "cacheRead", label: t("usageLedger.bucketCacheRead"), value: current.cached, color: U_ACCENT },
      { key: "input", label: t("usageLedger.bucketInput"), value: current.input, color: U_INK },
      { key: "cacheWrite", label: t("usageLedger.bucketCacheWrite"), value: current.cacheCreation, color: U_INK_2 },
      { key: "output", label: t("usageLedger.bucketOutput"), value: current.output, color: U_INK_3 },
    ];
    rows.sort((a, b) => b.value - a.value);
    return rows.map((row) => ({ ...row, share: row.value / totals }));
  }, [current, totals, t]);

  const breakdownRows = useMemo<RankedDatum[]>(() => {
    if (dim === "agent") {
      const sum = agents.reduce((s, row) => s + row.output, 0);
      return agents.map((row) => ({
        key: row.agentId,
        label: row.agentId === "other" ? t("usageLedger.other") : row.agentId,
        value: row.output,
        share: sum > 0 ? row.output / sum : null,
        hint: t("usageLedger.sessionsCount", { count: row.turns }),
        color: row.agentId === "other" ? U_FAINT : undefined,
      }));
    }
    return models.map((row) => {
      const share = totals > 0 ? totalTokens(row) / totals : null;
      const cost =
        costs && priceIndex ? costOfBuckets(row, rateForModel(row.modelId, priceIndex)) : null;
      const sessions = t("usageLedger.sessionsCount", { count: row.turns });
      const hint =
        cost?.usd != null && cost.usd > 0 ? `${sessions} · ${formatUsd(cost.usd)}` : sessions;
      return {
        key: row.modelId,
        label: row.modelId === "other" ? t("usageLedger.other") : row.modelId,
        value: totalTokens(row),
        share,
        hint,
        color: row.modelId === "other" ? U_FAINT : undefined,
      };
    });
  }, [dim, agents, models, totals, costs, priceIndex, t]);

  const peak = useMemo(() => {
    let best: { key: string; total: number } | null = null;
    for (const day of daily) {
      const total = totalTokens(day);
      if (total <= 0) continue;
      if (!best || total > best.total) best = { key: day.key, total };
    }
    return best;
  }, [daily]);

  const rangeLabel =
    rangeDays === 7
      ? t("usageLedger.last7")
      : rangeDays === 30
        ? t("usageLedger.last30")
        : t("usageLedger.last90");

  const changeRange = (days: RangeDays) => {
    setRangeDays(days);
    setBucketTouched(false);
  };

  // 导出 CSV(原生另存为;成功 toast 可一键 reveal 所在文件夹)。
  const [csvExporting, setCsvExporting] = useState(false);
  const handleExportCsv = async () => {
    if (csvExporting) return;
    setCsvExporting(true);
    const preparing = showToast({ text: t("usageLedger.csvPreparing"), tone: "info", duration: 30_000 });
    try {
      const { fileName, savedPath } = await exportUsageCsv();
      dismissToast(preparing);
      showToast({
        text: t("usageLedger.csvOk", { name: fileName }),
        tone: "ok",
        duration: 8000,
        action: savedPath
          ? { label: t("usageLedger.csvReveal"), run: () => void revealInFileManager(savedPath) }
          : undefined,
      });
    } catch (e) {
      dismissToast(preparing);
      showToast({
        text: t("usageLedger.csvError", { error: e instanceof Error ? e.message : String(e) }),
        tone: "error",
      });
    } finally {
      setCsvExporting(false);
    }
  };

  return (
    <div className="usage-analysis">
      <div className="usage-analysis__toolbar">
        <div className="usage-analysis__ranges" role="tablist" aria-label={rangeLabel}>
          {RANGES.map((days) => (
            <button
              key={days}
              type="button"
              role="tab"
              aria-selected={rangeDays === days}
              className={`composer-mode-chip${rangeDays === days ? " is-active" : ""}`}
              onClick={() => changeRange(days)}
            >
              {days === 7
                ? t("usageLedger.last7")
                : days === 30
                  ? t("usageLedger.last30")
                  : t("usageLedger.last90")}
            </button>
          ))}
        </div>
        {costs?.fetchedAt != null && (
          <span className="usage-analysis__sub">
            {t("usageLedger.priceUpdated", {
              when: new Date(costs.fetchedAt).toLocaleDateString(bcp47),
            })}
          </span>
        )}
        {hasAnyRecords && (
          <button
            type="button"
            className="composer-mode-chip usage-analysis__export"
            title={t("usageLedger.exportCsv")}
            aria-label={t("usageLedger.exportCsv")}
            disabled={csvExporting}
            onClick={() => void handleExportCsv()}
          >
            <Download size={11} strokeWidth={2.25} aria-hidden />
            {t("usageLedger.exportCsv")}
          </button>
        )}
      </div>

      {!hasAnyRecords ? (
        <div className="usage-analysis__empty">
          <p>{t("usageLedger.emptyTitle")}</p>
          <p>{t("usageLedger.emptyNote")}</p>
        </div>
      ) : (
        <>
          <section className={`usage-analysis__hero-card${cache != null ? " has-cache" : ""}`}>
            <div className="usage-analysis__hero-main">
              <div className="usage-analysis__cell-label">{t("usageLedger.tileTotal")}</div>
              <div className="usage-analysis__hero">
                <span className="usage-analysis__total">{formatTokenCount(totals)}</span>
                <DeltaBadge
                  delta={previousTotal > 0 ? delta : totals > 0 ? null : 0}
                  newLabel={t("usageLedger.deltaNew")}
                  title={t("usageLedger.vsPrevious")}
                />
              </div>
              {costs && costs.usd > 0 && (
                <div className="usage-analysis__cost-inline">
                  {formatUsd(costs.usd)}
                  {costs.coverage < 1 ? ` · ${t("usageLedger.coverage", { pct: pct(costs.coverage) })}` : ""}
                </div>
              )}
              {costs && costs.usd > 0 && (
                <p className="usage-analysis__cost-note">{t("usageLedger.costNote")}</p>
              )}
            </div>
            {cache != null && (
              <div className="usage-analysis__hero-cache">
                <RingMeter
                  ratio={cache}
                  valueText={pct(cache, CACHE_HIT_DIGITS)}
                  caption={t("usageLedger.cacheHitCaption")}
                />
                <div className="usage-analysis__hero-cache-copy">
                  <div className="usage-analysis__hero-cache-title">
                    {cache >= 0.5 ? t("usageLedger.cacheHeroHigh") : t("usageLedger.cacheHeroLow")}
                  </div>
                  <p className="usage-analysis__sub">
                    {t("usageLedger.cacheHeroDesc", {
                      cached: formatTokenCount(current.cached),
                      fresh: formatTokenCount(fresh),
                    })}
                  </p>
                </div>
              </div>
            )}
          </section>

          <section className="usage-analysis__strip">
            <StatCell
              label={t("usageLedger.tileTurns")}
              value={String(current.turns)}
              hint={
                current.turns > 0
                  ? t("usageLedger.avgPerTurn", {
                      tokens: formatTokenCount(Math.round(totals / current.turns)),
                    })
                  : undefined
              }
            />
            <StatCell
              label={t("usageLedger.tileActiveDays")}
              value={String(current.days)}
              hint={t("usageLedger.avgPerDay", {
                tokens: formatTokenCount(
                  current.days > 0 ? Math.round(totals / current.days) : 0,
                ),
              })}
            />
            <StatCell
              label={t("usageLedger.tileGenTime")}
              value={formatDurationCompact(current.ms)}
            />
            <StatCell
              label={t("usageLedger.tilePeak")}
              value={peak ? sliceDay(peak.key) : "—"}
              hint={peak ? formatTokenCount(peak.total) : undefined}
            />
          </section>

          <Panel
            title={t("usageLedger.trendTitle")}
            actions={
              <div className="usage-analysis__trend-actions">
                <span className="usage-analysis__legend">
                  <span>
                    <i style={{ background: U_ACCENT }} />
                    {t("usageLedger.bucketCacheRead")}
                  </span>
                  <span>
                    <i style={{ background: U_INK }} />
                    {t("usageLedger.freshTokens")}
                  </span>
                </span>
                <div className="usage-analysis__ranges" role="group">
                  {(["day", "week"] as const).map((id) => (
                    <button
                      key={id}
                      type="button"
                      className={`composer-mode-chip${effectiveBucket === id ? " is-active" : ""}`}
                      onClick={() => {
                        setBucket(id);
                        setBucketTouched(true);
                      }}
                    >
                      {id === "day" ? t("usageLedger.bucketDay") : t("usageLedger.bucketWeek")}
                    </button>
                  ))}
                </div>
              </div>
            }
          >
            <TrendChart
              data={trendData}
              label={t("usageLedger.trendTitle")}
              cacheLabel={t("usageLedger.bucketCacheRead")}
              freshLabel={t("usageLedger.freshTokens")}
              emptyLabel={t("usageLedger.trendEmpty")}
            />
          </Panel>

          <div className="usage-analysis__split">
            <Panel title={t("usageLedger.compositionTitle")} hint={t("usageLedger.compositionHint")}>
              <RankedBars data={compositionRows} emptyLabel={t("usageLedger.emptyBreakdown")} />
            </Panel>
            <Panel
              title={t("usageLedger.rhythmTitle")}
              hint={
                busiestHour != null
                  ? `${t("usageLedger.rhythmHint")} ${t("usageLedger.rhythmPeak", {
                      hour: String(busiestHour).padStart(2, "0"),
                    })}`
                  : t("usageLedger.rhythmHint")
              }
            >
              <HourHeatmap
                matrix={hourGrid}
                weekdayLabels={weekdays}
                legendLess={t("usageLedger.heatLess")}
                legendMore={t("usageLedger.heatMore")}
                formatValue={(value) => t("usageLedger.rhythmValue", { count: value })}
                formatAria={(weekday, hour, value) =>
                  t("usageLedger.rhythmCell", {
                    weekday: weekdays[weekday] ?? "",
                    hour: String(hour).padStart(2, "0"),
                    count: value,
                  })
                }
              />
            </Panel>
          </div>

          <Panel
            title={t("usageLedger.distributionTitle")}
            actions={
              <div className="usage-analysis__ranges" role="tablist">
                {(["model", "agent"] as const).map((id) => (
                  <button
                    key={id}
                    type="button"
                    role="tab"
                    aria-selected={dim === id}
                    className={`composer-mode-chip${dim === id ? " is-active" : ""}`}
                    onClick={() => setDim(id)}
                  >
                    {id === "model" ? t("usageLedger.byModel") : t("usageLedger.byAgent")}
                  </button>
                ))}
              </div>
            }
          >
            <RankedBars
              data={breakdownRows}
              showRank
              emptyLabel={t("usageLedger.emptyBreakdown")}
            />
          </Panel>
        </>
      )}
    </div>
  );
}

/** 弹窗形态:贴边窄条 / detached 窗没有标签行,分析内容以大弹窗兜底。 */
export function UsageAnalysisDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
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
        <div className="project-dialog__body">
          <UsageAnalysisBody />
        </div>
      </div>
    </div>
  );
}
