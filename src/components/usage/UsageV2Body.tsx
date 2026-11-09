import { useEffect, useMemo, useState } from "react";
import { Download } from "lucide-react";
import { useT } from "../../lib/i18n";
import { agentDisplayName, agentInkStyle } from "../../lib/agentInk";
import { exportUsageCsv, useUsageLedger } from "../../lib/usageLedger";
import { aggregateInsights, type InsightRange, type SessionRow } from "../../lib/usageInsights";
import { fetchPriceCatalog, type RateIndex } from "../../lib/tokenPrices";
import { revealInFileManager } from "../../lib/api";
import { dismissToast, showToast } from "../../lib/toast";
import { Seg } from "./primitives";
import { UsageHero } from "./UsageHero";
import { UsageTrend } from "./UsageTrend";
import { UsageWhere } from "./UsageWhere";
import { UsageAgents } from "./UsageAgents";
import { UsageActivity } from "./UsageActivity";
import { UsageRhythm } from "./UsageRhythm";
import { UsageSessions } from "./UsageSessions";
import { UsageSessionDetail } from "./UsageSessionDetail";
import { UsageBackfill } from "./UsageBackfill";
import { UsageLegend } from "./UsageLegend";

/**
 * Token usage page v2 — the body shared by the 「Token 用量」 pseudo tab, the dock
 * page and the detached dialog. Top bar (range · agent filter · CSV), then the
 * sections in the order of the canvas board「v2 · Token 用量」. All numbers come
 * from usageInsights.aggregateInsights over the local ledger.
 */

const RANGES: readonly InsightRange[] = [1, 7, 30, 90, 180];

export function UsageV2Body({ onOpenSession }: { onOpenSession?: (id: string) => void }) {
  const t = useT();
  const ledger = useUsageLedger();
  const [range, setRange] = useState<InsightRange>(30);
  const [agent, setAgent] = useState<string | null>(null);
  const [prices, setPrices] = useState<RateIndex | null>(null);
  const [exporting, setExporting] = useState(false);
  const [detail, setDetail] = useState<SessionRow | null>(null);

  useEffect(() => {
    let alive = true;
    void fetchPriceCatalog().then((index) => {
      if (alive) setPrices(index);
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
    () => aggregateInsights(ledger, { range, agent, prices }),
    [ledger, range, agent, prices],
  );

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
        action: savedPath ? { label: t("usageLedger.csvReveal"), run: () => void revealInFileManager(savedPath) } : undefined,
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
        <h1>{t("usageV2.title")}</h1>
        <Seg
          label={t("usageV2.range.label")}
          value={range}
          options={RANGES.map((r) => ({ value: r, label: t(`usageV2.range.${r}`) }))}
          onChange={setRange}
        />
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
        <button type="button" className="uv2-ghost" onClick={() => void exportCsv()} disabled={exporting}>
          <Download size={13} strokeWidth={1.5} aria-hidden="true" />
          {t("usageV2.export")}
        </button>
      </div>

      {!ins.hasData ? (
        <p className="uv2-empty">{t("usageV2.empty")}</p>
      ) : (
        <>
          <UsageHero ins={ins} />
          <UsageTrend ins={ins} />
          <UsageWhere ins={ins} />
          <UsageAgents ins={ins} />
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
        </>
      )}
    </div>
  );
}
