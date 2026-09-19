import { ChevronRight } from "lucide-react";
import {
  averageTtftMs,
  cacheHitPercent,
  formatDurationCompact,
  formatExactTokenCount,
  formatTokenCount,
  formatTokensPerSecond,
  hasTokenActivity,
  outputTokensPerSecond,
  totalTokens,
  type SessionStats,
} from "../lib/sessionStats";

/**
 * Whole-dialog statistics inside the Usage card: two collapsible groups whose
 * headers carry the at-a-glance summary. Session-scoped on purpose — per-turn
 * figures are not duplicated here — and a dialog that never ran a step renders
 * nothing at all.
 */

type StatRow = { label: string; value: string };

function StatRows({ rows }: { rows: StatRow[] }) {
  return (
    <dl className="session-stats__rows">
      {rows.map((row) => (
        <div className="session-stats__row" key={row.label}>
          <dt>{row.label}</dt>
          <dd>{row.value}</dd>
        </div>
      ))}
    </dl>
  );
}

function StatGroup({
  name,
  summary,
  rows,
}: {
  name: string;
  summary: string;
  rows: StatRow[];
}) {
  return (
    <details className="session-stats__group">
      <summary className="session-stats__summary">
        <span className="session-stats__name">
          <ChevronRight className="session-stats__chev" size={12} aria-hidden />
          {name}
        </span>
        <span className="session-stats__value">{summary}</span>
      </summary>
      <StatRows rows={rows} />
    </details>
  );
}

export function SessionStatsSection({ stats }: { stats: SessionStats }) {
  const hasTokens = hasTokenActivity(stats);
  if (stats.steps === 0 && !hasTokens) return null;

  /** The step in flight contributes estimated tokens; mark those figures `~`. */
  const live = stats.liveOutputTokens > 0;
  const tps = outputTokensPerSecond(stats);
  const tpsText = tps == null ? null : `${live ? "~" : ""}${formatTokensPerSecond(tps)} tok/s`;
  const ttft = averageTtftMs(stats);
  const counts = `${stats.turns} 轮 ${stats.steps} 步`;
  const timeSummary = tpsText == null ? counts : `${counts} · ${tpsText}`;
  // A group with no timed figure would open empty, so it stays a plain reading
  // instead of a disclosure.
  const hasTimedFigure =
    stats.llmMs > 0 || stats.toolMs > 0 || stats.ttftSteps > 0 || stats.decodeMs > 0;

  const timeRows: StatRow[] = [];
  if (stats.llmMs > 0) {
    timeRows.push({ label: "模型用时", value: formatDurationCompact(stats.llmMs) });
  }
  if (stats.toolMs > 0) {
    timeRows.push({ label: "工具调用用时", value: formatDurationCompact(stats.toolMs) });
  }
  if (ttft != null) {
    timeRows.push({ label: "首 token 平均（TTFT）", value: formatDurationCompact(ttft) });
  }
  if (tpsText != null) {
    timeRows.push({ label: "输出速度（TPS）", value: tpsText });
  }

  const total = totalTokens(stats) + stats.liveOutputTokens;
  const cacheHit = cacheHitPercent(stats);
  const usageSummary =
    cacheHit == null
      ? `${live ? "~" : ""}${formatTokenCount(total)} tok`
      : `${live ? "~" : ""}${formatTokenCount(total)} tok · 缓存命中 ${cacheHit}%`;

  const usageRows: StatRow[] = [];
  if (cacheHit != null) usageRows.push({ label: "缓存命中", value: `${cacheHit}%` });
  usageRows.push({
    label: "未缓存输入",
    value: `${formatExactTokenCount(stats.uncachedInputTokens)} tok`,
  });
  usageRows.push({
    label: "缓存读取",
    value: `${formatExactTokenCount(stats.cacheReadTokens)} tok`,
  });
  if (stats.cacheWriteTokens !== 0) {
    usageRows.push({
      label: "缓存写入",
      value: `${formatExactTokenCount(stats.cacheWriteTokens)} tok`,
    });
  }
  usageRows.push({ label: "输出", value: `${formatExactTokenCount(stats.outputTokens)} tok` });
  if (live) {
    usageRows.push({
      label: "输出（流式中）",
      value: `~${formatExactTokenCount(stats.liveOutputTokens)} tok`,
    });
  }

  return (
    <div className="session-stats">
      {hasTimedFigure ? (
        <StatGroup name="会话统计" summary={timeSummary} rows={timeRows} />
      ) : (
        <div className="session-stats__reading">
          <span className="session-stats__name">会话统计</span>
          <span className="session-stats__value">{timeSummary}</span>
        </div>
      )}
      {hasTokens && <StatGroup name="Token 用量" summary={usageSummary} rows={usageRows} />}
    </div>
  );
}
