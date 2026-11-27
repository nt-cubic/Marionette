/**
 * UsageLimits - the Limits tab of the Token usage page.
 *
 * Two honest blocks and nothing invented:
 *   1. price coverage - how much of the period's token volume the models.dev
 *      catalog could price, plus the agent-reported and peak-surcharge parts;
 *   2. subscription limits - the engine's quota windows (used %, reset time,
 *      balance) with a stale-snapshot badge when the numbers are not live.
 *
 * Marionette has no per-provider usage-limit endpoint beyond those two, so when
 * neither has data the tab says so instead of drawing an empty meter.
 */
import { useState } from "react";
import type { EngineQuotaEntry } from "../../lib/api";
import { useT } from "../../lib/i18n";
import { pctLevel } from "../../lib/quotaRail";
import type { UsageInsights } from "../../lib/usageInsights";
import {
  buildScheduleAfterResetPayload,
  parseTrustworthyReset,
  type ScheduleAfterResetPayload,
} from "../../lib/usageSchedule";
import { SectionHead, fmtPct, fmtUsd } from "./primitives";

export type UsageQuotaState = {
  entries: EngineQuotaEntry[];
  stale: boolean;
  savedAt: number | null;
};

function whenText(value: string | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

function usedPct(window: EngineQuotaEntry["windows"][number]): number | null {
  if (typeof window.used === "number" && Number.isFinite(window.used)) {
    return Math.min(100, Math.max(0, window.used));
  }
  if (typeof window.remaining === "number" && Number.isFinite(window.remaining)) {
    return Math.min(100, Math.max(0, 100 - window.remaining));
  }
  return null;
}

function QuotaRow({
  entry,
  allowSchedule,
  nowMs,
  onSchedule,
}: {
  entry: EngineQuotaEntry;
  allowSchedule: boolean;
  nowMs: number;
  onSchedule?: (payload: ScheduleAfterResetPayload) => void;
}) {
  const t = useT();
  const [scheduleKey, setScheduleKey] = useState<number | null>(null);
  const [scheduleText, setScheduleText] = useState("");
  const windows = entry.windows.filter((window) => window.name || usedPct(window) != null);
  return (
    <div className="uv2-limits__prov">
      <div className="uv2-limits__provhead">
        <b>{entry.name || entry.provider}</b>
        <em>
          {entry.provider}
          {entry.user ? ` \u00b7 ${entry.user}` : ""}
        </em>
        {entry.balance && <span className="uv2-limits__bal">{t("usageV2.limits.quota.balance", { balance: entry.balance })}</span>}
      </div>
      {windows.map((window, index) => {
        const used = usedPct(window);
        const reset = whenText(window.resetsAt);
        const resetMs = parseTrustworthyReset(window.resetsAt, nowMs);
        const canSchedule = allowSchedule && resetMs != null && !!onSchedule;
        const open = scheduleKey === index;
        return (
          <div className="uv2-limits__winwrap" key={`${window.name}-${index}`}>
            <div className="uv2-limits__win">
              <span className="uv2-limits__wname">{window.name || entry.provider}</span>
              {used == null ? (
                <span className="uv2-limits__wna faint">{"\u2014"}</span>
              ) : (
                <i className="uv2-limits__bar" aria-hidden="true">
                  <s className={`is-${pctLevel(used)}`} style={{ width: `${used.toFixed(1)}%` }} />
                </i>
              )}
              <span className="num">{used == null ? "" : fmtPct(used / 100, 0)}</span>
              <span className="uv2-limits__wreset faint">
                {reset ? t("usageV2.limits.quota.resets", { when: reset }) : ""}
              </span>
              {canSchedule && (
                <button
                  type="button"
                  className="uv2-limits__schedbtn"
                  aria-expanded={open}
                  onClick={() => {
                    setScheduleKey(open ? null : index);
                    setScheduleText("");
                  }}
                >
                  {t("usageV2.schedule.action")}
                </button>
              )}
            </div>
            {canSchedule && open && (
              <form
                className="uv2-limits__sched"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (resetMs == null) return;
                  const payload = buildScheduleAfterResetPayload(
                    {
                      provider: entry.provider,
                      entryName: entry.name || entry.provider,
                      windowName: window.name || entry.provider,
                      resetsAtMs: resetMs,
                      usedPct: used,
                    },
                    scheduleText,
                    nowMs,
                  );
                  if (!payload) return;
                  onSchedule?.(payload);
                  setScheduleKey(null);
                  setScheduleText("");
                }}
              >
                <label>
                  <span>{t("usageV2.schedule.prompt")}</span>
                  <textarea
                    value={scheduleText}
                    rows={2}
                    autoFocus
                    placeholder={t("usageV2.schedule.placeholder")}
                    onChange={(event) => setScheduleText(event.target.value)}
                  />
                </label>
                <div className="uv2-limits__schedrow">
                  <span className="faint">
                    {resetMs != null
                      ? t("usageV2.schedule.at", {
                          when: whenText(new Date(resetMs).toISOString()) ?? "",
                        })
                      : ""}
                  </span>
                  <button type="submit" className="uv2-ghost" disabled={!scheduleText.trim()}>
                    {t("usageV2.schedule.confirm")}
                  </button>
                  <button
                    type="button"
                    className="uv2-ghost"
                    onClick={() => {
                      setScheduleKey(null);
                      setScheduleText("");
                    }}
                  >
                    {t("common.cancel")}
                  </button>
                </div>
                <p className="uv2-limits__schednote">{t("usageV2.schedule.hint")}</p>
              </form>
            )}
          </div>
        );
      })}
    </div>
  );
}

export function UsageLimits({
  ins,
  quota,
  onScheduleAfterReset,
}: {
  ins: UsageInsights;
  quota: UsageQuotaState;
  onScheduleAfterReset?: (payload: ScheduleAfterResetPayload) => void;
}) {
  const t = useT();
  const rows = quota.entries.filter(
    (entry) => entry.windows.length > 0 || (entry.balance != null && entry.balance.trim() !== ""),
  );
  const staleWhen = quota.savedAt != null ? whenText(new Date(quota.savedAt).toString()) : null;
  const nowMs = Date.now();
  const usesCustomPrice = ins.agents.some((agent) => agent.models.some((model) => model.pricedBy === "custom"));

  return (
    <div className="uv2-limits">
      <section className="uv2-sec">
        <SectionHead title={t("usageV2.limits.coverage.title")} meta={t("usageV2.limits.coverage.meta")} />
        {ins.usd.value == null ? (
          <p className="uv2-limits__empty">{t("usageV2.limits.coverage.unavailable")}</p>
        ) : (
          <div className="uv2-limits__grid">
            <div>
              <b>{fmtPct(ins.usd.coverage, 0)}</b>
              <span>{t("usageV2.limits.coverage.rate")}</span>
            </div>
            <div>
              <b>{fmtUsd(ins.usd.value)}</b>
              <span>{t("usageV2.limits.coverage.cost")}</span>
            </div>
            <div>
              <b>{ins.usd.reported > 0 ? fmtUsd(ins.usd.reported) : "\u2014"}</b>
              <span>{t("usageV2.limits.coverage.reported")}</span>
            </div>
            <div>
              <b>{ins.usd.peakExtra > 0 ? fmtUsd(ins.usd.peakExtra) : "\u2014"}</b>
              <span>{t("usageV2.limits.coverage.peak")}</span>
            </div>
          </div>
        )}
        <p className="uv2-limits__note">
          {usesCustomPrice ? t("usageV2.limits.coverage.customNote") : t("usageV2.limits.coverage.note")}
        </p>
      </section>

      <section className="uv2-sec">
        <SectionHead
          title={t("usageV2.limits.quota.title")}
          meta={t("usageV2.limits.quota.meta")}
          right={
            quota.stale ? (
              <span className="uv2-tag uv2-tag--estimated">
                {staleWhen
                  ? t("usageV2.limits.quota.stale", { when: staleWhen })
                  : t("usageV2.limits.quota.staleNoWhen")}
              </span>
            ) : undefined
          }
        />
        {rows.length === 0 ? (
          <p className="uv2-limits__empty">{t("usageV2.limits.quota.empty")}</p>
        ) : (
          <div className="uv2-limits__provs">
            {rows.map((entry) => (
              <QuotaRow
                key={`${entry.provider}\u0000${entry.name}`}
                entry={entry}
                allowSchedule={!quota.stale}
                nowMs={nowMs}
                onSchedule={onScheduleAfterReset}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
