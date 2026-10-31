import { useMemo } from "react";
import {
  formatDurationCompact,
  formatTokenCount,
} from "../lib/sessionStats";
import {
  summarizeLedger,
  topAgents7,
  useUsageLedger,
  type LedgerAggregate,
} from "../lib/usageLedger";
import { useT } from "../lib/i18n";

/**
 * 「Token 总览」:跨会话的本地用量台账(今天 / 近 7 天 / 近 30 天)。
 * 数据来自 usageLedger(每轮 turn/complete 增量记账,localStorage 持久),
 * 与 Usage 卡的会话内统计(SessionStatsSection)互补;没有任何记录时
 * 整段不渲染。
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

export function UsageLedgerSection() {
  const t = useT();
  const ledger = useUsageLedger();
  const summary = useMemo(() => summarizeLedger(ledger), [ledger]);
  const agents = useMemo(() => topAgents7(summary), [summary]);
  if (summary.last30.turns === 0) return null;

  return (
    <section className="context-card" id="context-usage-ledger">
      <div className="context-card__heading">
        <span>{t("usageLedger.title")}</span>
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
    </section>
  );
}
