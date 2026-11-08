import { ChevronRight, RefreshCw } from "lucide-react";
import { memo, useCallback, useState } from "react";
import { agentInkStyle } from "../lib/agentInk";
import { useT } from "../lib/i18n";
import {
  buildQuotaView,
  compactReset,
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

const OPEN_KEY = "marionette-quota-open";

function readOpen(): string[] {
  try {
    const raw = localStorage.getItem(OPEN_KEY);
    const list = raw ? (JSON.parse(raw) as unknown) : [];
    return Array.isArray(list) ? list.filter((v): v is string => typeof v === "string") : [];
  } catch {
    return [];
  }
}

function writeOpen(keys: string[]): void {
  try {
    localStorage.setItem(OPEN_KEY, JSON.stringify(keys));
  } catch {
    /* per-viewer convenience only */
  }
}

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

/**
 * Another source, folded to one line (name · tightest % · its reset); click opens
 * every window of it with its own bar and reset time. Open state is remembered.
 */
function OtherGroup({
  group,
  label,
  open,
  onToggle,
}: {
  group: UsageGroup;
  label: string;
  open: boolean;
  onToggle: (key: string) => void;
}) {
  const t = useT();
  const value = otherValue(group, t);
  const tight = tightestWindow(group);
  const reset = compactReset(resetHint(tight));
  return (
    <div className={`quota-other${open ? " is-open" : ""}`}>
      <button
        type="button"
        className="rail-ln quota-other__head"
        aria-expanded={open}
        onClick={() => onToggle(group.key)}
      >
        <ChevronRight size={12} strokeWidth={1.75} className="quota-other__chev" aria-hidden />
        <span className="rail-ln__text">{label}</span>
        {!open && reset && <span className="quota-other__reset">{reset}</span>}
        <span className={`rail-ln__r is-${value.level}`}>{value.text}</span>
      </button>
      {open && (
        <div className="quota-other__body">
          <GroupRows group={group} tight={tight} />
        </div>
      )}
    </div>
  );
}

export const QuotaSection = memo(function QuotaSection({
  usage,
  agentId,
  modelId,
  onRefresh,
}: QuotaSectionProps) {
  const t = useT();
  const [openKeys, setOpenKeys] = useState<string[]>(readOpen);
  const toggle = useCallback((key: string) => {
    setOpenKeys((cur) => {
      const next = cur.includes(key) ? cur.filter((k) => k !== key) : [...cur, key];
      writeOpen(next);
      return next;
    });
  }, []);
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
      {view.others.map((group) => (
        <OtherGroup
          key={group.key}
          group={group}
          label={group.key === "agent" ? t("rail.ownSub", { agent: usage.agentLabel }) : group.name}
          open={openKeys.includes(group.key)}
          onToggle={toggle}
        />
      ))}
      {empty && <p className="rail-empty">{t("rail.quotaEmpty")}</p>}
    </section>
  );
});
