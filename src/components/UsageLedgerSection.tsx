import { useEffect, useMemo, useState } from "react";
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
 * 整段不渲染。「查看分析」打开 UsageAnalysisDialog 大面板;Ctrl K 的
 * 同名动作经 openUsageAnalysis() 走同一扇门(setDocTabOpener 同款)。
 */

/** Ctrl K → 打开分析面板的唯一入口(App 挂载点注册,组件卸载即失效)。 */
let externalOpener: (() => void) | null = null;

export function openUsageAnalysis(): void {
  externalOpener?.();
}

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

/** 近 7 天迷你堆叠条(输出/读取/缓存读/缓存写,同分析面板的系列色)。 */
function Sparkline({ ledger }: { ledger: UsageLedger }) {
  const t = useT();
  const series = useMemo(() => dailySeries(ledger, 7), [ledger]);
  const max = Math.max(1, ...series.map((d) => d.input + d.output + d.cached + d.cacheCreation));
  const total7 = series.reduce(
    (sum, d) => sum + d.input + d.output + d.cached + d.cacheCreation,
    0,
  );
  return (
    <div>
      <div className="usage-ledger__spark" aria-hidden>
        {series.map((day) => {
          const total = day.input + day.output + day.cached + day.cacheCreation;
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
              className="usage-ledger__spark-col"
              title={`${day.key} · ${formatTokenCount(total)}`}
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
        <span>{t("usageLedger.sparkTotal", { tokens: formatTokenCount(total7) })}</span>
      </div>
    </div>
  );
}

export function UsageLedgerSection() {
  const t = useT();
  const ledger = useUsageLedger();
  const [analysisOpen, setAnalysisOpen] = useState(false);
  const summary = useMemo(() => summarizeLedger(ledger), [ledger]);
  const agents = useMemo(() => topAgents7(summary), [summary]);

  useEffect(() => {
    externalOpener = () => setAnalysisOpen(true);
    return () => {
      externalOpener = null;
    };
  }, []);

  if (summary.last30.turns === 0 && !analysisOpen) return null;

  return (
    <section className="context-card" id="context-usage-ledger">
      <div className="context-card__heading">
        <span>{t("usageLedger.title")}</span>
        <button
          type="button"
          className="pill-action pill-action--sm"
          onClick={() => setAnalysisOpen(true)}
        >
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
