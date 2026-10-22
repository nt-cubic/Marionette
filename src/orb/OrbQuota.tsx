/**
 * 余额 section of the orb panel — a QuotaBar-style list of every provider the
 * translation engine reports: subscription quota windows (meter + 剩 X% +
 * 重置时间) and key balances (¥/$ strings). Row markup/marking reuses the
 * Usage panel's `.usage-row` / `.usage-meter__track` classes from app.css
 * (loaded in the orb window too); the math mirrors usage.ts `engineQuotaRows`.
 */

import type { EngineQuotaEntry } from "../lib/api";
import { formatClock, formatQuotaReset } from "./orbCapture";

export type OrbQuotaSnapshot = {
  entries: EngineQuotaEntry[];
  updatedAt: number;
  /** True when the engine is down / the fetch failed / nothing came back. */
  failed: boolean;
};

export type OrbQuotaProps = {
  snapshot: OrbQuotaSnapshot | null;
  busy: boolean;
  onRefresh: () => void;
};

type QuotaWindowRow = {
  id: string;
  label: string;
  /** Used percent for the meter bar, mirroring usage.ts. */
  percentage: number | null;
  detail: string | null;
  remaining: number | null;
};

/** usage.ts engineQuotaRows math, with the warn/hot tone kept for the meter. */
function windowRows(entry: EngineQuotaEntry): QuotaWindowRow[] {
  const rows: QuotaWindowRow[] = [];
  for (const w of entry.windows) {
    if (w.used == null && w.remaining == null && !w.resetsAt) continue;
    // Row value = 剩 X% (when the vendor reports it); detail line = reset time.
    const reset = formatQuotaReset(w.resetsAt);
    rows.push({
      id: `${entry.provider}:${w.name}`,
      label: w.name,
      percentage: w.used != null ? Math.max(0, Math.min(100, w.used)) : null,
      detail: reset ? `${reset} 重置` : null,
      remaining: w.remaining ?? null,
    });
  }
  return rows;
}

function meterTone(remaining: number | null): "warn" | "hot" | "ok" {
  if (remaining == null) return "ok";
  if (remaining <= 5) return "hot";
  if (remaining <= 20) return "warn";
  return "ok";
}

export function OrbQuota({ snapshot, busy, onRefresh }: OrbQuotaProps) {
  const entries = snapshot?.entries ?? [];
  const hasData = !snapshot?.failed && entries.length > 0;

  return (
    <section className="orb-panel__section">
      <div className="orb-panel__section-head">
        <h3>余额</h3>
        <span className="orb-panel__section-meta">
          {snapshot ? `更新于 ${formatClock(snapshot.updatedAt)}` : ""}
          <button
            type="button"
            className="orb-panel__icon-btn"
            disabled={busy}
            title="刷新余额"
            onClick={onRefresh}
          >
            {busy ? "…" : "↻"}
          </button>
        </span>
      </div>
      {!hasData ? (
        <p className="orb-panel__empty">翻译引擎未运行（余额不可用）</p>
      ) : (
        <div className="orb-usage">
          {entries.map((entry) => {
            const windows = windowRows(entry);
            const title = entry.name || entry.provider;
            return (
              <div key={entry.provider || title} className="orb-usage__provider">
                <div className="orb-usage__provider-title">
                  <strong>{title}</strong>
                  {entry.user ? <span className="orb-usage__user">{entry.user}</span> : null}
                </div>
                {windows.length === 0 && !entry.balance ? (
                  <span className="orb-panel__empty orb-panel__empty--inline">暂无配额数据</span>
                ) : null}
                {entry.balance ? (
                  <div className="usage-row">
                    <span>余额</span>
                    <strong>{entry.balance}</strong>
                  </div>
                ) : null}
                <div className="usage-list">
                  {windows.map((w) => {
                    const tone = meterTone(w.remaining);
                    return (
                      <div
                        key={w.id}
                        className={`usage-row${tone === "ok" ? "" : ` usage-row--${tone}`}`}
                      >
                        <span>{w.label}</span>
                        {w.remaining != null ? <strong>剩 {Math.round(w.remaining)}%</strong> : null}
                        {w.detail ? <span className="usage-row__detail">{w.detail}</span> : null}
                        {w.percentage != null ? (
                          <div className="usage-meter__track">
                            <span style={{ width: `${w.percentage}%` }} />
                          </div>
                        ) : null}
                      </div>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}
