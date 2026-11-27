/**
 * Token usage page - the body shared by the pseudo tab, the dock page and the
 * detached dialog.
 *
 * Information hierarchy (T3 transplant):
 *   compact top bar (period / agent filter) -> persistent Cost | Tokens | Limits
 *   tabs -> a status strip (period, price coverage, tightest limit) -> one
 *   headline summary, one trend chart and the provider/model share band -> a
 *   collapsed "detailed analysis" layer holding every pre-existing section
 *   (agent/model table, token/time destination, tools & files, rhythm, sessions
 *   with per-turn detail, history backfill, CSV export and the provenance
 *   footer).
 *
 * All numbers still come from usageInsights.aggregateInsights over the local
 * ledger; no data or analysis was removed, only re-layered.
 */
import { useEffect, useMemo, useState } from "react";
import { ChevronRight, Download } from "lucide-react";
import { useT } from "../../lib/i18n";
import { agentDisplayName, agentInkStyle } from "../../lib/agentInk";
import { exportUsageCsv, useUsageLedger } from "../../lib/usageLedger";
import { aggregateInsights, type InsightRange, type SessionRow } from "../../lib/usageInsights";
import { fetchPriceCatalog, type RateIndex } from "../../lib/tokenPrices";
import {
  hasCustomPrices,
  loadUsageOverrides,
  saveUsageOverrides,
  type UsageOverrides as UsageOverridesState,
} from "../../lib/usageOverrides";
import { stashUsageScheduleRequest, type ScheduleAfterResetPayload } from "../../lib/usageSchedule";
import { engineQuota, revealInFileManager } from "../../lib/api";
import { cachedEngineQuota, cacheEngineQuota } from "../../lib/quotaCache";
import { dismissToast, showToast } from "../../lib/toast";
import { fmtPct } from "./primitives";
import { UsageHero } from "./UsageHero";
import { UsageTrend } from "./UsageTrend";
import { UsageShares } from "./UsageShares";
import { UsageLimits, type UsageQuotaState } from "./UsageLimits";
import { UsageOverrides } from "./UsageOverrides";
import { UsageWhere } from "./UsageWhere";
import { UsageAgents } from "./UsageAgents";
import { UsageActivity } from "./UsageActivity";
import { UsageRhythm } from "./UsageRhythm";
import { UsageSessions } from "./UsageSessions";
import { UsageSessionDetail } from "./UsageSessionDetail";
import { UsageBackfill } from "./UsageBackfill";
import { UsageLegend } from "./UsageLegend";
import {
  readMoreOpen,
  readUsageView,
  storeMoreOpen,
  storeUsageView,
  type UsageView,
} from "./viewState";

const RANGES: readonly InsightRange[] = [1, 7, 30, 90, 180];

const VIEWS: readonly UsageView[] = ["cost", "tokens", "limits"];

/** Best-effort quota snapshot: live rows when the engine is up, else the disk cache. */
function initialQuota(): UsageQuotaState {
  const cached = cachedEngineQuota();
  return cached
    ? { entries: cached.entries, stale: true, savedAt: cached.savedAt }
    : { entries: [], stale: false, savedAt: null };
}

export function UsageV2Body({
  onOpenSession,
  onScheduleAfterReset,
}: {
  onOpenSession?: (id: string) => void;
  /** Host hook: enqueue this through the existing scheduled-send path. */
  onScheduleAfterReset?: (payload: ScheduleAfterResetPayload) => void;
}) {
  const t = useT();
  const ledger = useUsageLedger();
  const [range, setRange] = useState<InsightRange>(30);
  const [agent, setAgent] = useState<string | null>(null);
  const [prices, setPrices] = useState<RateIndex | null>(null);
  const [overrides, setOverrides] = useState<UsageOverridesState>(loadUsageOverrides);
  const [exporting, setExporting] = useState(false);
  const [detail, setDetail] = useState<SessionRow | null>(null);
  const [view, setView] = useState<UsageView>(readUsageView);
  const [more, setMore] = useState<boolean>(readMoreOpen);
  const [quota, setQuota] = useState<UsageQuotaState>(initialQuota);

  useEffect(() => {
    let alive = true;
    void fetchPriceCatalog().then((index) => {
      if (alive) setPrices(index);
    });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    let alive = true;
    void engineQuota(false).then((result) => {
      if (!alive) return;
      if (result.entries.length === 0) {
        const cached = cachedEngineQuota();
        setQuota(
          cached
            ? { entries: cached.entries, stale: true, savedAt: cached.savedAt }
            : { entries: [], stale: false, savedAt: null },
        );
        return;
      }
      const cached = cachedEngineQuota();
      cacheEngineQuota(result.entries);
      setQuota({
        entries: result.entries,
        stale: result.stale,
        savedAt: result.stale ? (cached?.savedAt ?? null) : Date.now(),
      });
    });
    return () => {
      alive = false;
    };
  }, []);

  // The filter chips list every agent seen in the widest window, so a chip does not
  // vanish when the range is narrowed to a day it did not run.
  const allAgents = useMemo(
    () => aggregateInsights(ledger, { range: 180, prices: null }).agents.map((a) => a.agentId),
    [ledger],
  );
  const ins = useMemo(
    () => aggregateInsights(ledger, { range, agent, prices, overrides }),
    [ledger, range, agent, prices, overrides],
  );
  const customPrices = hasCustomPrices(overrides);

  const changeView = (next: UsageView) => {
    setView(next);
    storeUsageView(next);
  };

  const toggleMore = () => {
    setMore((prev) => {
      const next = !prev;
      storeMoreOpen(next);
      return next;
    });
  };

  const changeOverrides = (next: UsageOverridesState) => {
    setOverrides(next);
    saveUsageOverrides(next);
  };

  const scheduleAfterReset = (payload: ScheduleAfterResetPayload) => {
    const when = new Date(payload.atMs).toLocaleString([], {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
    if (onScheduleAfterReset) {
      onScheduleAfterReset(payload);
      showToast({ text: t("usageV2.schedule.scheduled", { when }), tone: "ok", duration: 9000 });
      return;
    }
    const saved = stashUsageScheduleRequest(payload);
    showToast({
      text: saved
        ? t("usageV2.schedule.saved", { when })
        : t("usageV2.schedule.saveFailed"),
      tone: saved ? "info" : "error",
      duration: saved ? 10_000 : undefined,
    });
  };

  const tightest = useMemo(() => {
    let best: { name: string; used: number } | null = null;
    for (const entry of quota.entries) {
      for (const window of entry.windows) {
        const used =
          typeof window.used === "number" && Number.isFinite(window.used)
            ? Math.min(100, Math.max(0, window.used))
            : typeof window.remaining === "number" && Number.isFinite(window.remaining)
              ? Math.min(100, Math.max(0, 100 - window.remaining))
              : null;
        if (used == null) continue;
        if (!best || used > best.used) {
          best = { name: window.name ? `${entry.name} \u00b7 ${window.name}` : entry.name, used };
        }
      }
    }
    return best;
  }, [quota.entries]);

  const exportCsv = async () => {
    if (exporting) return;
    setExporting(true);
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
      showToast({ text: t("usageLedger.csvError", { error: e instanceof Error ? e.message : String(e) }), tone: "error" });
    } finally {
      setExporting(false);
    }
  };

  return (
    <div className="uv2">
      <div className="uv2-top">
        <div className="uv2-top__row">
          <h1>{t("usageV2.title")}</h1>
          <span className="uv2-seg" role="group" aria-label={t("usageV2.range.label")}>
            {RANGES.map((r) => (
              <button
                key={r}
                type="button"
                aria-pressed={range === r}
                className={range === r ? "is-on" : undefined}
                onClick={() => setRange(r)}
              >
                {t(`usageV2.range.${r}`)}
              </button>
            ))}
          </span>
          {allAgents.length > 1 && (
            <div className="uv2-chips" role="group" aria-label={t("usageV2.filter.label")}>
              <button type="button" className={`uv2-chip${agent == null ? " is-on" : ""}`} aria-pressed={agent == null} onClick={() => setAgent(null)}>
                {t("usageV2.filter.all")}
              </button>
              {allAgents.map((id) => (
                <button
                  key={id}
                  type="button"
                  className={`uv2-chip${agent === id ? " is-on" : ""}`}
                  aria-pressed={agent === id}
                  style={agentInkStyle(id)}
                  onClick={() => setAgent(agent === id ? null : id)}
                >
                  <i aria-hidden="true" />
                  {agentDisplayName(id)}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="uv2-views" role="tablist" aria-label={t("usageV2.view.label")}>
          {VIEWS.map((id) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={view === id}
              className={view === id ? "is-on" : undefined}
              onClick={() => changeView(id)}
            >
              {t(`usageV2.view.${id}`)}
            </button>
          ))}
        </div>
      </div>

      {!ins.hasData && view !== "limits" ? (
        <p className="uv2-empty">{t("usageV2.empty")}</p>
      ) : (
        <>
          <div className="uv2-status">
            <span>{t("usageV2.status.period", { days: range })}</span>
            <span>
              {customPrices || (prices && prices.byModel.size > 0)
                ? t("usageV2.status.coverage", { pct: fmtPct(ins.usd.coverage, 0) })
                : t("usageV2.status.coverageNone")}
            </span>
            <span>
              {tightest
                ? t("usageV2.status.limit", { name: tightest.name, pct: fmtPct(tightest.used / 100, 0) })
                : t("usageV2.status.limitNone")}
              {quota.stale && quota.entries.length > 0 && <em>{t("usageV2.status.stale")}</em>}
            </span>
            {customPrices && <span className="uv2-status__custom">{t("usageV2.status.customPrices")}</span>}
          </div>

          {view === "limits" ? (
            <UsageLimits ins={ins} quota={quota} onScheduleAfterReset={scheduleAfterReset} />
          ) : (
            <>
              <UsageHero ins={ins} view={view} customPrices={customPrices} />
              <UsageShares ins={ins} />
              <UsageTrend ins={ins} />
            </>
          )}

          {ins.hasData && (
            <div className="uv2-more">
            <button type="button" className="uv2-more__toggle" aria-expanded={more} onClick={toggleMore}>
              <ChevronRight size={13} strokeWidth={1.5} className={more ? "is-open" : undefined} aria-hidden="true" />
              <b>{more ? t("usageV2.more.collapse") : t("usageV2.more.expand")}</b>
              <span className="uv2-more__meta">{t("usageV2.more.meta")}</span>
            </button>
            {more && (
              <div className="uv2-more__body">
                <div className="uv2-more__bar">
                  <button type="button" className="uv2-ghost" onClick={() => void exportCsv()} disabled={exporting}>
                    <Download size={13} strokeWidth={1.5} aria-hidden="true" />
                    {t("usageV2.export")}
                  </button>
                </div>
                <UsageOverrides ins={ins} overrides={overrides} onChange={changeOverrides} />
                <UsageAgents ins={ins} />
                <UsageWhere ins={ins} />
                <UsageActivity ins={ins} />
                <UsageRhythm ins={ins} />
                <UsageSessions ins={ins} onOpenSession={onOpenSession} onOpenDetail={setDetail} />
                {detail && (
                  <UsageSessionDetail
                    sessionId={detail.id}
                    label={detail.label}
                    agentId={detail.agentId}
                    onClose={() => setDetail(null)}
                  />
                )}
                <UsageBackfill range={range} agent={agent} />
                <UsageLegend />
              </div>
            )}
          </div>
          )}
        </>
      )}
    </div>
  );
}
