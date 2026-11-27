/**
 * UsageShares - the first-screen "who used the most" band.
 *
 * Two short ranked lists: agents and models, each with a token share bar. This
 * is deliberately not the full agent/model table (UsageAgents); it only carries
 * the top few rows so the first screen answers "largest provider/model share"
 * without opening anything. The detailed table stays in the expandable layer.
 */
import { useT } from "../../lib/i18n";
import { agentDisplayName, agentInkStyle } from "../../lib/agentInk";
import type { UsageInsights } from "../../lib/usageInsights";
import { SectionHead, fmtPct, fmtTokens, fmtUsd } from "./primitives";

const TOP_AGENTS = 3;
const TOP_MODELS = 4;

type ShareRow = {
  key: string;
  label: string;
  sub?: string;
  total: number;
  share: number;
  usd: number | null;
  agentId?: string;
};

function ShareList({ title, rows }: { title: string; rows: readonly ShareRow[] }) {
  return (
    <div className="uv2-shares__list">
      <h3>{title}</h3>
      {rows.map((row) => (
        <div className="uv2-shares__row" key={row.key} style={row.agentId ? agentInkStyle(row.agentId) : undefined}>
          <span className="uv2-shares__name" title={row.label}>
            <b>{row.label}</b>
            {row.sub && <em>{row.sub}</em>}
          </span>
          <i className="uv2-shares__bar" aria-hidden="true">
            <s style={{ width: `${Math.min(100, row.share * 100).toFixed(2)}%` }} />
          </i>
          <span className="num">{fmtPct(row.share, 0)}</span>
          <span className="num faint">{fmtTokens(row.total)}</span>
          <span className="num faint">{row.usd == null ? "\u2014" : fmtUsd(row.usd)}</span>
        </div>
      ))}
    </div>
  );
}

export function UsageShares({ ins }: { ins: UsageInsights }) {
  const t = useT();
  const total = ins.totals.total;

  const agentRows: ShareRow[] = ins.agents.slice(0, TOP_AGENTS).map((agent) => ({
    key: agent.agentId,
    label: agentDisplayName(agent.agentId),
    total: agent.total,
    share: total > 0 ? agent.total / total : 0,
    usd: agent.usd,
    agentId: agent.agentId,
  }));

  const modelRows: ShareRow[] = ins.agents
    .flatMap((agent) =>
      agent.models.map((model) => ({
        key: `${agent.agentId}\u0000${model.modelId}`,
        label: model.modelId,
        sub: agentDisplayName(agent.agentId),
        total: model.total,
        share: total > 0 ? model.total / total : 0,
        usd: model.usd,
        agentId: agent.agentId,
      })),
    )
    .sort((a, b) => b.total - a.total)
    .slice(0, TOP_MODELS);

  const topModel = modelRows[0];
  const topAgent = agentRows[0];
  const top = total > 0 ? (topModel ?? topAgent) : null;

  return (
    <section className="uv2-sec uv2-shares">
      <SectionHead
        title={t("usageV2.shares.title")}
        meta={top ? t("usageV2.shares.top", { name: top.label, pct: fmtPct(top.share, 0) }) : t("usageV2.shares.meta")}
      />
      {total <= 0 || (agentRows.length === 0 && modelRows.length === 0) ? (
        <p className="uv2-shares__none">{t("usageV2.shares.none")}</p>
      ) : (
        <div className="uv2-shares__cols">
          <ShareList title={t("usageV2.shares.agents")} rows={agentRows} />
          <ShareList title={t("usageV2.shares.models")} rows={modelRows} />
        </div>
      )}
    </section>
  );
}
