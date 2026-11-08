/**
 * UsageAgents — the per agent · model table of the Token usage page (v2).
 * Each agent is one expandable full-row button carrying its ink colour
 * (--agent-ink); model sub-rows scale their token bars against the same
 * agent maximum. Speed percentiles are per agent, so model rows dash them.
 */
import { Fragment, useState } from "react";
import { ChevronRight } from "lucide-react";
import { useT } from "../../lib/i18n";
import { agentDisplayName, agentInkStyle } from "../../lib/agentInk";
import type { UsageInsights } from "../../lib/usageInsights";
import { SectionHead, Sparkline, fmtMs, fmtPct, fmtTokens, fmtUsd } from "./primitives";

/** Token-bar width in % of the busiest agent's total (0 when there is nothing). */
function barPct(total: number, max: number): number {
  return max > 0 ? Math.min(100, (total / max) * 100) : 0;
}

function TokenBar({ total, max }: { total: number; max: number }) {
  return (
    <span className="uv2-agents__tok">
      <i style={{ width: `${barPct(total, max).toFixed(2)}%` }} />
      <b>{fmtTokens(total)}</b>
    </span>
  );
}

/** Cost cell; the dash carries a tooltip explaining why nothing could be priced. */
function UsdCell({ usd, noRate }: { usd: number | null; noRate: string }) {
  return (
    <span className="num" title={usd == null ? noRate : undefined}>
      {fmtUsd(usd)}
    </span>
  );
}

export function UsageAgents({ ins }: { ins: UsageInsights }) {
  const t = useT();
  const agents = ins.agents;
  // The busiest agent starts expanded, but only when it really has several models.
  const [open, setOpen] = useState<ReadonlySet<string>>(() => {
    const first = agents[0];
    return first && first.models.length > 1 ? new Set([first.agentId]) : new Set<string>();
  });

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const maxTotal = agents.reduce((m, a) => Math.max(m, a.total), 0);
  const noRate = t("usageV2.agents.noRate");

  return (
    <section className="uv2-sec uv2-agents">
      <SectionHead title={t("usageV2.agents.title")} meta={t("usageV2.agents.meta")} />
      {agents.length === 0 ? (
        <p className="uv2-agents__empty">{t("usageV2.agents.empty")}</p>
      ) : (
        <div>
          <div className="uv2-agents__th">
            <span>{t("usageV2.agents.col.agent")}</span>
            <span>{t("usageV2.agents.col.tokens")}</span>
            <span>{t("usageV2.agents.col.trend")}</span>
            <span>{t("usageV2.agents.col.hit")}</span>
            <span>{t("usageV2.agents.col.cost")}</span>
            <span>{t("usageV2.agents.col.turns")}</span>
            <span>{t("usageV2.agents.col.ttft")}</span>
            <span>{t("usageV2.agents.col.tps")}</span>
          </div>
          {agents.map((row) => {
            const isOpen = open.has(row.agentId);
            const single = row.models.length === 1 ? row.models[0] : undefined;
            const name = agentDisplayName(row.agentId);
            return (
              <Fragment key={row.agentId}>
                <button
                  type="button"
                  className="uv2-agents__row"
                  style={agentInkStyle(row.agentId)}
                  aria-expanded={isOpen}
                  onClick={() => toggle(row.agentId)}
                >
                  <span className="uv2-agents__who">
                    {row.models.length > 1 && (
                      <ChevronRight
                        className={`uv2-agents__chev${isOpen ? " is-open" : ""}`}
                        size={12}
                        strokeWidth={1.5}
                        aria-hidden="true"
                      />
                    )}
                    <b>{name}</b>
                    {single && <em>{single.modelId}</em>}
                  </span>
                  <TokenBar total={row.total} max={maxTotal} />
                  <span
                    className="uv2-agents__spark"
                    role="img"
                    aria-label={t("usageV2.agents.trend", { agent: name })}
                  >
                    <Sparkline values={row.series} color="var(--agent-ink)" width={88} height={20} area={false} />
                  </span>
                  <span className="num">{fmtPct(row.hit)}</span>
                  <UsdCell usd={row.usd} noRate={noRate} />
                  <span className="num">{row.turns}</span>
                  <span className="num">{row.ttftP50 == null ? "—" : fmtMs(row.ttftP50)}</span>
                  <span className="num">{row.tpsP50 == null ? "—" : Math.round(row.tpsP50)}</span>
                </button>
                {isOpen &&
                  row.models.map((m) => (
                    <div key={m.modelId} className="uv2-agents__sub" style={agentInkStyle(row.agentId)}>
                      <span className="uv2-agents__who uv2-agents__who--sub">
                        <em>{m.modelId}</em>
                      </span>
                      <TokenBar total={m.total} max={maxTotal} />
                      <span className="uv2-agents__spark" />
                      <span className="num">{fmtPct(m.hit)}</span>
                      <UsdCell usd={m.usd} noRate={noRate} />
                      <span className="num">{m.turns}</span>
                      <span className="num faint">—</span>
                      <span className="num faint">—</span>
                    </div>
                  ))}
              </Fragment>
            );
          })}
        </div>
      )}
    </section>
  );
}
