/**
 * UsageBackfill — the「历史对话(本机日志)」band of the Token usage page.
 *
 * One button scans every agent CLI's own on-disk store (Rust usage_backfill,
 * cached per file). Sessions the live ledger already counts — matched by the
 * agent-reported session id against Marionette's session list — are listed
 * with a badge but not summed, so the two views never double-count.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { History, RefreshCw } from "lucide-react";
import { useT } from "../../lib/i18n";
import { agentDisplayName, agentInkStyle } from "../../lib/agentInk";
import { CHAT_PROJECT_ID, listProjects, listSessions } from "../../lib/api";
import {
  foldBackfill,
  scanBackfill,
  scaleComposition,
  type BackfillReport,
} from "../../lib/usageBackfill";
import { fmtMs, fmtTokens, fmtUsd, SectionHead } from "./primitives";
import { COMP_COLOR } from "./UsageSessionDetail";

const TOP_ROWS = 20;

export function UsageBackfill({ range, agent }: { range: number; agent: string | null }) {
  const t = useT();
  const [report, setReport] = useState<BackfillReport | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nativeIds, setNativeIds] = useState<Set<string>>(new Set());

  // Marionette's own session list carries the native ids the live ledger counts.
  useEffect(() => {
    let alive = true;
    void (async () => {
      const projects = await listProjects().catch(() => []);
      const lists = await Promise.all([
        listSessions(CHAT_PROJECT_ID).catch(() => []),
        ...projects.map((project) => listSessions(project.id).catch(() => [])),
      ]);
      if (!alive) return;
      const ids = new Set<string>();
      for (const session of lists.flat()) {
        for (const native of session.nativeSessionIds ?? []) {
          if (native) {
            ids.add(native);
            ids.add(`${session.agentId ?? ""}\u0000${native}`);
          }
        }
      }
      setNativeIds(ids);
    })().catch(() => {
      /* dedup falls back to "everything counts" */
    });
    return () => {
      alive = false;
    };
  }, []);

  const runScan = useCallback((force: boolean) => {
    if (scanning) return;
    setScanning(true);
    setError(null);
    void scanBackfill(force)
      .then((result) => {
        setReport(result);
      })
      .catch((e) => {
        setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        setScanning(false);
      });
  }, [scanning]);

  const folded = useMemo(
    () => (report ? foldBackfill(report, { days: range, agent, nativeIds }) : null),
    [report, range, agent, nativeIds],
  );

  const sourceLine = (source: NonNullable<BackfillReport["sources"][number]>): string => {
    const name = agentDisplayName(source.agent);
    if (source.error) return t("usageV2.backfill.sourceErr", { agent: name, error: source.error });
    return t("usageV2.backfill.sourceOk", {
      agent: name,
      sessions: source.sessions,
      turns: source.turns,
      ms: fmtMs(source.elapsedMs),
    });
  };

  return (
    <section className="uv2-sec uv2-backfill">
      <SectionHead
        title={t("usageV2.backfill.title")}
        meta={report && folded ? t("usageV2.backfill.totals", {
          sessions: folded.sessions,
          turns: folded.turns,
          tokens: fmtTokens(folded.tokens),
        }) : t("usageV2.backfill.meta")}
        right={
          <button
            type="button"
            className="uv2-ghost"
            onClick={() => runScan(report != null)}
            disabled={scanning}
          >
            {scanning ? (
              <RefreshCw size={13} strokeWidth={1.5} className="uv2-spin" aria-hidden="true" />
            ) : (
              <History size={13} strokeWidth={1.5} aria-hidden="true" />
            )}
            {scanning
              ? t("usageV2.backfill.scanning")
              : report != null
                ? t("usageV2.backfill.rescan")
                : t("usageV2.backfill.scan")}
          </button>
        }
      />
      {error && <p className="uv2-backfill__err">{t("usageV2.backfill.failed", { error })}</p>}
      {folded && folded.rows.length === 0 && !scanning && (
        <p className="uv2-backfill__empty">{t("usageV2.backfill.empty")}</p>
      )}
      {folded && folded.rows.length > 0 && (
        <>
          <div className="uv2-backfill__rows">
            {folded.rows.slice(0, TOP_ROWS).map((row) => (
              <div
                className={`uv2-backfill__row${row.duplicate ? " is-dup" : ""}`}
                key={`${row.session.agent}/${row.session.nativeId}`}
                style={agentInkStyle(row.session.agent)}
                title={row.session.cwd ?? undefined}
              >
                <span className="uv2-backfill__t">
                  <span className="uv2-backfill__label">{row.session.title ?? row.session.nativeId}</span>
                  <span className="uv2-backfill__sub">
                    {agentDisplayName(row.session.agent)}
                    {row.session.compactions.length > 0 ? ` · ⤾ ${row.session.compactions.length}` : ""}
                    {row.duplicate ? ` · ${t("usageV2.backfill.duplicate")}` : ""}
                  </span>
                  {row.session.composition && (
                    <span className="uv2-comp__bar uv2-comp__bar--mini" aria-hidden="true">
                      {scaleComposition(row.session.composition).parts
                        .filter((part) => part.tokens > 0)
                        .map((part) => (
                          <i key={part.key} style={{ flexGrow: part.tokens, background: COMP_COLOR[part.key] }} />
                        ))}
                    </span>
                  )}
                </span>
                <span className="num">{row.turns.toLocaleString()}</span>
                <span className="num">{fmtTokens(row.tokens)}</span>
                <span className="num faint">
                  {row.reportedUsd > 0 ? t("usageV2.backfill.repUsd", { usd: fmtUsd(row.reportedUsd) }) : "—"}
                </span>
              </div>
            ))}
          </div>
          <div className="uv2-backfill__srcs">
            {report!.sources.map((source) => (
              <span key={source.agent} className={source.error ? "is-err" : undefined}>
                {sourceLine(source)}
              </span>
            ))}
          </div>
          <p className="uv2-backfill__note">{t("usageV2.backfill.note")}</p>
        </>
      )}
    </section>
  );
}
