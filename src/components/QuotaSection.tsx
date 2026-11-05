import { RefreshCw } from "lucide-react";
import { memo } from "react";
import { agentInkStyle } from "../lib/agentInk";
import { useT } from "../lib/i18n";
import {
  buildQuotaView,
  formatPct,
  groupBalance,
  pctLevel,
  periodLabel,
  resetHint,
  tightestWindow,
  usagePeriod,
  windowBalance,
  type QuotaPrimary,
  type UsageGroup,
} from "../lib/quotaRail";
import type { UsageSnapshot, UsageWindow } from "../lib/types";

type QuotaSectionProps = {
  usage: UsageSnapshot;
  agentId?: string;
  /** Model the current conversation runs on — decides who is billed this turn. */
  modelId?: string | null;
  onRefresh: () => void;
};

function PrimaryBlock({
  primary,
  agentId,
  agentLabel,
}: {
  primary: QuotaPrimary;
  agentId?: string;
  agentLabel: string;
}) {
  const t = useT();
  const { group, own, unknown } = primary;
  const tight = tightestWindow(group);
  const barWindows = group.windows.filter((w) => w.percentage != null);
  const balance = barWindows.length === 0 ? groupBalance(group) : null;
  const name = own ? t("rail.ownSub", { agent: agentLabel }) : group.name;
  const note = unknown ? t("rail.quotaUnknown") : own ? null : t("rail.quotaVia", { agent: agentLabel });
  const reset = resetHint(tight);
  // Re-mounting on a new billing source replays the one-shot bar flash.
  const flashKey = `${agentId ?? ""}|${group.key}|${own ? "own" : "bridge"}`;

  return (
    <div key={flashKey} className="quota-cur" style={agentInkStyle(agentId)}>
      <div className="quota-cur__name">
        <span className="quota-cur__title">{name}</span>
        {note && <span className="quota-cur__note">{note}</span>}
        {reset && <span className="quota-cur__reset">{reset}</span>}
      </div>
      {balance != null ? (
        <div className="quota-cur__balance">
          <span className="quota-cur__big">{balance}</span>
          <span className="quota-cur__note">{t("rail.balance")}</span>
        </div>
      ) : (
        barWindows.map((w) => {
          const isTight = w === tight;
          const pct = w.percentage as number;
          const level = isTight ? pctLevel(pct) : "ok";
          return (
            <div
              className={`quota-wr${isTight ? " is-tight" : ""} is-${level}`}
              key={w.id}
              title={w.detail ?? undefined}
            >
              <span>{periodLabel(usagePeriod(w, group.name))}</span>
              <i>
                <span style={{ width: `${Math.min(100, Math.max(0, pct))}%` }} />
              </i>
              <em>{formatPct(pct)}</em>
            </div>
          );
        })
      )}
    </div>
  );
}

function otherValue(
  group: UsageGroup,
  t: ReturnType<typeof useT>,
): { text: string; level: "ok" | "warn" | "hot" } {
  if (group.peak != null) {
    const level = pctLevel(group.peak);
    return { text: level === "hot" ? t("rail.full") : formatPct(group.peak), level };
  }
  const balance = groupBalance(group);
  return { text: balance ? t("rail.balanceOf", { balance }) : "", level: "ok" };
}

function groupTitle(group: UsageGroup): string | undefined {
  const lines = group.windows
    .map((w: UsageWindow) => {
      const bal = windowBalance(w);
      return [w.label, w.percentage != null ? formatPct(w.percentage) : bal, w.detail].filter(Boolean).join(" · ");
    })
    .filter(Boolean);
  return lines.length > 0 ? lines.join("\n") : undefined;
}

export const QuotaSection = memo(function QuotaSection({
  usage,
  agentId,
  modelId,
  onRefresh,
}: QuotaSectionProps) {
  const t = useT();
  const view = buildQuotaView({ windows: usage.windows, agentLabel: usage.agentLabel, modelId });
  const empty = !view.primary && view.others.length === 0;
  const refreshTitle = [
    t("rail.quotaRefresh"),
    [usage.note, usage.refreshedAt].filter((part, i, all): part is string => Boolean(part) && all.indexOf(part) === i).join(" · "),
  ]
    .filter(Boolean)
    .join("\n");

  return (
    <section className="rail-sec" id="context-usage" aria-label={t("rail.quota")}>
      <div className="rail-sh">
        <span>{t("rail.quota")}</span>
        <span className="rail-act">
          <button type="button" className="rail-ib" title={refreshTitle} aria-label={t("rail.quotaRefresh")} onClick={onRefresh}>
            <RefreshCw size={12} strokeWidth={1.5} />
          </button>
        </span>
      </div>
      {view.primary && <PrimaryBlock primary={view.primary} agentId={agentId} agentLabel={usage.agentLabel} />}
      {view.others.map((group) => {
        const value = otherValue(group, t);
        const own = group.key === "agent";
        return (
          <div className="rail-ln" key={group.key} title={groupTitle(group)}>
            <span className="rail-ln__text">{own ? t("rail.ownSub", { agent: usage.agentLabel }) : group.name}</span>
            <span className={`rail-ln__r is-${value.level}`}>{value.text}</span>
          </div>
        );
      })}
      {empty && <p className="rail-empty">{t("rail.quotaEmpty")}</p>}
    </section>
  );
});
