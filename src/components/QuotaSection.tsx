import { RefreshCw } from "lucide-react";
import { memo, useState } from "react";
import { agentInkStyle } from "../lib/agentInk";
import { useT } from "../lib/i18n";
import {
  buildQuotaView,
  compactReset,
  formatPct,
  groupBalance,
  pctLevel,
  periodHours,
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

/** One window: period · bar · used % · when it resets (today = time only). */
function MeterRow({ window: w, source, tight }: { window: UsageWindow; source: string; tight: boolean }) {
  const t = useT();
  const pct = w.percentage as number;
  const level = tight ? pctLevel(pct) : "ok";
  const full = resetHint(w);
  const reset = compactReset(full);
  return (
    <div className={`quota-wr${tight ? " is-tight" : ""} is-${level}`} title={w.detail ?? undefined}>
      <span>{periodLabel(usagePeriod(w, source))}</span>
      <i>
        <span style={{ width: `${Math.min(100, Math.max(0, pct))}%` }} />
      </i>
      <em>{formatPct(pct)}</em>
      <small className="quota-wr__reset" title={full ? t("rail.resetAt", { when: full }) : undefined}>
        {reset ?? ""}
      </small>
    </div>
  );
}

/** A balance-only window (e.g. DeepSeek ¥28.30): label · money. */
function BalanceRow({ window: w, source }: { window: UsageWindow; source: string }) {
  const money = windowBalance(w);
  if (!money) return null;
  return (
    <div className="quota-wr quota-wr--balance" title={w.detail ?? undefined}>
      <span>{periodLabel(usagePeriod(w, source))}</span>
      <em>{money}</em>
    </div>
  );
}

function GroupRows({ group, tight }: { group: UsageGroup; tight: UsageWindow | null }) {
  return (
    <>
      {group.windows.map((w) =>
        w.percentage != null ? (
          <MeterRow key={w.id} window={w} source={group.name} tight={w === tight} />
        ) : (
          <BalanceRow key={w.id} window={w} source={group.name} />
        ),
      )}
    </>
  );
}

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
  const hasBars = group.windows.some((w) => w.percentage != null);
  const balance = hasBars ? null : groupBalance(group);
  const name = own ? t("rail.ownSub", { agent: agentLabel }) : group.name;
  const note = unknown ? t("rail.quotaUnknown") : own ? null : t("rail.quotaVia", { agent: agentLabel });
  // Re-mounting on a new billing source replays the one-shot bar flash.
  const flashKey = `${agentId ?? ""}|${group.key}|${own ? "own" : "bridge"}`;

  return (
    <div key={flashKey} className="quota-cur" style={agentInkStyle(agentId)}>
      <div className="quota-cur__name">
        <span className="quota-cur__title">{name}</span>
        {note && <span className="quota-cur__note">{note}</span>}
      </div>
      {balance != null ? (
        <div className="quota-cur__balance">
          <span className="quota-cur__big">{balance}</span>
          <span className="quota-cur__note">{t("rail.balance")}</span>
        </div>
      ) : (
        <GroupRows group={group} tight={tight} />
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

/** Concentric arcs, one per window: the shortest window outermost (Apple-activity style). */
function Dial({ group, tight }: { group: UsageGroup; tight: UsageWindow | null }) {
  const bars = group.windows
    .filter((w) => w.percentage != null)
    .map((w) => ({ w, h: periodHours(usagePeriod(w, group.name)) ?? 9999 }))
    .sort((x, y) => x.h - y.h)
    .slice(0, 3);
  const size = 52;
  const c = size / 2;
  return (
    <svg className="quota-dial__svg" width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden="true">
      {bars.map(({ w }, i) => {
        const r = 24 - i * 4.5;
        const len = 2 * Math.PI * r;
        const pct = Math.min(100, Math.max(0, w.percentage as number));
        const level = pctLevel(pct);
        return (
          <g key={w.id}>
            <circle className="quota-dial__track" cx={c} cy={c} r={r} />
            {pct > 0 && (
              <circle
                className={`quota-dial__arc is-${level}${w === tight ? " is-tight" : ""}`}
                cx={c}
                cy={c}
                r={r}
                strokeDasharray={`${((pct / 100) * len).toFixed(2)} ${len.toFixed(2)}`}
                transform={`rotate(-90 ${c} ${c})`}
              />
            )}
          </g>
        );
      })}
      {bars.length === 0 && <circle className="quota-dial__track is-balance" cx={c} cy={c} r={24} />}
    </svg>
  );
}

/**
 * Every other source as a dial in a three-up grid (no boxes): rings = its windows,
 * centre = the tightest %, or the balance; under it the name and the next reset.
 * Picking a dial lays its windows out as bars in one strip under the grid.
 */
function OtherDials({ groups, agentLabel }: { groups: UsageGroup[]; agentLabel: string }) {
  const t = useT();
  const [picked, setPicked] = useState<string | null>(null);
  const current = groups.find((g) => g.key === picked) ?? null;
  return (
    <div className="quota-dials">
      <div className="quota-dials__grid" role="list">
        {groups.map((group) => {
          const tight = tightestWindow(group);
          const value = otherValue(group, t);
          const balance = group.peak == null ? groupBalance(group) : null;
          const reset = compactReset(resetHint(tight));
          const label = group.key === "agent" ? t("rail.ownSub", { agent: agentLabel }) : group.name;
          const on = group.key === picked;
          return (
            <button
              key={group.key}
              type="button"
              role="listitem"
              className={`quota-dial${on ? " is-on" : ""} is-${value.level}`}
              aria-pressed={on}
              title={groupTitle(group)}
              onClick={() => setPicked(on ? null : group.key)}
            >
              <span className="quota-dial__ring">
                <Dial group={group} tight={tight} />
                <b className="quota-dial__value">
                  {balance != null
                    ? "¥"
                    : group.peak == null
                      ? "—"
                      : group.peak >= 99.5
                        ? t("rail.full")
                        : `${Math.round(group.peak)}%`}
                </b>
              </span>
              <span className="quota-dial__name">{label}</span>
              <span className="quota-dial__reset">{balance ?? reset ?? " "}</span>
            </button>
          );
        })}
      </div>
      {current && (
        <div className="quota-dials__detail">
          <div className="quota-dials__detail-head">
            <span>{current.key === "agent" ? t("rail.ownSub", { agent: agentLabel }) : current.name}</span>
          </div>
          <GroupRows group={current} tight={tightestWindow(current)} />
        </div>
      )}
    </div>
  );
}

function groupTitle(group: UsageGroup): string | undefined {
  const lines = group.windows
    .map((w: UsageWindow) => {
      const bal = windowBalance(w);
      const when = resetHint(w);
      return [w.label, w.percentage != null ? formatPct(w.percentage) : bal, when].filter(Boolean).join(" · ");
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
      {view.others.length > 0 && <OtherDials groups={view.others} agentLabel={usage.agentLabel} />}
      {empty && <p className="rail-empty">{t("rail.quotaEmpty")}</p>}
    </section>
  );
});
