import { useMemo, useState } from "react";
import {
  formatDurationCompact,
  formatTokenCount,
} from "../lib/sessionStats";
import {
  dailySeries,
  summarizeLedger,
  topAgents7,
  useUsageLedger,
  type LedgerAggregate,
  type UsageLedger,
} from "../lib/usageLedger";
import { useT } from "../lib/i18n";
import { UsageAnalysisDialog } from "./UsageAnalysisDialog";

/**
 * 「Token 总览」:跨会话的本地用量台账(今天 / 近 7 天 / 近 30 天)。
 * 数据来自 usageLedger(每轮 turn/complete 增量记账,localStorage 持久),
 * 与 Usage 卡的会话内统计(SessionStatsSection)互补;没有任何记录时
 * 整段不渲染。「查看分析」:主窗口由 App 传入 openVia(打开伪标签页);
 * 贴边/detached 不传,卡内以 UsageAnalysisDialog 弹窗兜底。
 */

function aggregateValue(
  agg: LedgerAggregate,
  t: ReturnType<typeof useT>,
): string {
  const parts = [
    `↓${formatTokenCount(agg.output)} ↑${formatTokenCount(agg.input)}`,
    t("usageLedger.turns", { count: agg.turns }),
  ];
  if (agg.ms > 0) parts.push(formatDurationCompact(agg.ms));
  return parts.join(" · ");
}

/** 近 7 天迷你堆叠条,可扫读(uiarc sparkline 的交互语法):hover 或键盘
 * (role=slider,←→/Home/End)读当天,读值行默认显示 7 天合计。 */
function Sparkline({ ledger }: { ledger: UsageLedger }) {
  const t = useT();
  const series = useMemo(() => dailySeries(ledger, 7), [ledger]);
  const [cursor, setCursor] = useState<number | null>(null);
  const max = Math.max(1, ...series.map((d) => d.input + d.output + d.cached + d.cacheCreation));
  const total7 = series.reduce(
    (sum, d) => sum + d.input + d.output + d.cached + d.cacheCreation,
    0,
  );
  const dayAt = (index: number) => series[Math.max(0, Math.min(index, series.length - 1))]!;
  const dayTotal = (d: (typeof series)[number]) =>
    d.input + d.output + d.cached + d.cacheCreation;
  return (
    <div>
      <div
        className="usage-ledger__spark"
        role="slider"
        aria-label={t("usageLedger.last7")}
        aria-valuemin={0}
        aria-valuemax={Math.max(0, series.length - 1)}
        aria-valuenow={cursor ?? series.length - 1}
        aria-valuetext={
          cursor != null
            ? `${dayAt(cursor).key} · ${formatTokenCount(dayTotal(dayAt(cursor)))}`
            : t("usageLedger.sparkTotal", { tokens: formatTokenCount(total7) })
        }
        tabIndex={0}
        onPointerMove={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          const ratio = (event.clientX - rect.left) / Math.max(1, rect.width);
          setCursor(Math.max(0, Math.min(series.length - 1, Math.floor(ratio * series.length))));
        }}
        onPointerLeave={() => setCursor(null)}
        onKeyDown={(event) => {
          const moves: Record<string, number> = {
            ArrowLeft: (cursor ?? series.length - 1) - 1,
            ArrowRight: (cursor ?? series.length - 1) + 1,
            Home: 0,
            End: series.length - 1,
          };
          if (event.key in moves) {
            event.preventDefault();
            setCursor(Math.max(0, Math.min(series.length - 1, moves[event.key]!)));
          } else if (event.key === "Escape") {
            setCursor(null);
          }
        }}
      >
        {series.map((day, index) => {
          const total = dayTotal(day);
          if (total <= 0) return <div key={day.key} className="usage-ledger__spark-col" />;
          const segs: Array<{ cls: string; value: number }> = [
            { cls: "out", value: day.output },
            { cls: "in", value: day.input },
            { cls: "cr", value: day.cached },
            { cls: "cw", value: day.cacheCreation },
          ];
          return (
            <div
              key={day.key}
              className={`usage-ledger__spark-col${cursor === index ? " is-cursor" : ""}`}
            >
              {segs
                .filter((seg) => seg.value > 0)
                .map((seg) => (
                  <div
                    key={seg.cls}
                    className={`usage-ledger__spark-seg usage-ledger__spark-seg--${seg.cls}`}
                    style={{ height: `${Math.max(2, (seg.value / max) * 100)}%` }}
                  />
                ))}
            </div>
          );
        })}
      </div>
      <div className="usage-ledger__spark-note">
        <span>{series[0]?.key.slice(5)}</span>
        <span>
          {cursor != null
            ? `${dayAt(cursor).key.slice(5)} · ${formatTokenCount(dayTotal(dayAt(cursor)))}`
            : t("usageLedger.sparkTotal", { tokens: formatTokenCount(total7) })}
        </span>
      </div>
    </div>
  );
}

export function UsageLedgerSection({ openVia }: { openVia?: () => void } = {}) {
  const t = useT();
  const ledger = useUsageLedger();
  const [analysisOpen, setAnalysisOpen] = useState(false);
  const summary = useMemo(() => summarizeLedger(ledger), [ledger]);
  const agents = useMemo(() => topAgents7(summary), [summary]);
  const openAnalysis = openVia ?? (() => setAnalysisOpen(true));

  if (summary.last30.turns === 0 && !analysisOpen) return null;

  return (
    <section className="context-card" id="context-usage-ledger">
      <div className="context-card__heading">
        <span>{t("usageLedger.title")}</span>
        <button type="button" className="pill-action pill-action--sm" onClick={openAnalysis}>
          {t("usageLedger.viewAnalysis")}
        </button>
      </div>
      <dl className="session-stats__rows">
        <div className="session-stats__row">
          <dt>{t("common.today")}</dt>
          <dd>{aggregateValue(summary.today, t)}</dd>
        </div>
        <div className="session-stats__row">
          <dt>{t("usageLedger.last7")}</dt>
          <dd>{aggregateValue(summary.last7, t)}</dd>
        </div>
        <div className="session-stats__row">
          <dt>{t("usageLedger.last30")}</dt>
          <dd>{aggregateValue(summary.last30, t)}</dd>
        </div>
      </dl>
      <Sparkline ledger={ledger} />
      {agents.length > 0 && (
        <details className="session-stats__group">
          <summary className="session-stats__summary">
            <span className="session-stats__name">{t("usageLedger.byAgent")}</span>
            <span className="session-stats__value">
              {formatTokenCount(agents.reduce((sum, row) => sum + row.output, 0))}
            </span>
          </summary>
          <dl className="session-stats__rows">
            {agents.map((row) => (
              <div className="session-stats__row" key={row.agentId}>
                <dt>{row.agentId}</dt>
                <dd>
                  {formatTokenCount(row.output)} · {t("usageLedger.turns", { count: row.turns })}
                </dd>
              </div>
            ))}
          </dl>
        </details>
      )}
      <small className="usage-updated">{t("usageLedger.hint")}</small>
      {analysisOpen && <UsageAnalysisDialog onClose={() => setAnalysisOpen(false)} />}
    </section>
  );
}
