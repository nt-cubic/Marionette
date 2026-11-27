import { useEffect, useState } from "react";
import type {
  SubagentCardSnapshot,
  SubagentObservationState,
} from "../lib/subagentObservation";
import "./SubagentObservationCard.css";

/**
 * Compact parent-card view of a delegated worker (P0-5 live action + P0-7 final
 * summary). Additive metadata only: it never collapses, replaces or proxies the
 * worker's own conversation.
 *
 * Every user-facing string is supplied by the caller through `labels`, so the
 * component has no i18n dependency and no hard-coded sentence. `snapshot` comes
 * from `observeSubagent(...).card`, i.e. it is computed from the same worker
 * event array that renders the worker transcript.
 */
export type SubagentObservationCardLabels = {
  /** One label per modelled state. */
  states: Record<SubagentObservationState, string>;
  /** "12 steps" - the caller decides wording and plural form. */
  steps: (count: number) => string;
  /** "12 tool calls" - summary card count. */
  tools: (count: number) => string;
  /** "1m 24s" - formats a millisecond span. */
  duration: (ms: number) => string;
  /** Live card, before the first tool frame arrives. */
  noActionYet: string;
  /** Finished card that produced no assistant text. */
  emptySummary: string;
  /** Waiting-for-input hint used when the ledger gave no reason. */
  waitingHint: string;
  /** Optional footer button label; the button only renders when onOpenWorker is set. */
  openWorker?: string;
};

export type SubagentObservationCardProps = {
  snapshot: SubagentCardSnapshot;
  labels: SubagentObservationCardLabels;
  onOpenWorker?: () => void;
  className?: string;
};

/** Keep the last one or two path segments; the full path stays in the tooltip. */
function displayPath(path: string): string {
  const normalized = path.replace(/\\/g, "/").trim();
  if (!normalized) return "";
  if (/^https?:\/\//i.test(normalized)) {
    return normalized.length > 64 ? `${normalized.slice(0, 63)}\u2026` : normalized;
  }
  const parts = normalized.split("/").filter(Boolean);
  const short =
    parts.length > 2 ? `\u2026/${parts.slice(-2).join("/")}` : normalized;
  return short.length > 64 ? `${short.slice(0, 63)}\u2026` : short;
}

export function SubagentObservationCard({
  snapshot,
  labels,
  onOpenWorker,
  className,
}: SubagentObservationCardProps) {
  const active = snapshot.active;
  const [nowMs, setNowMs] = useState(() => Date.now());

  // Live elapsed chip only. Terminal durations come from the snapshot, so a
  // finished card never needs a timer.
  useEffect(() => {
    if (!active) return;
    setNowMs(Date.now());
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active, snapshot.startedAtMs]);

  const liveMs =
    active && snapshot.startedAtMs != null
      ? Math.max(0, nowMs - snapshot.startedAtMs)
      : null;
  const durationMs = snapshot.durationMs ?? liveMs;

  const stateLabel = labels.states[snapshot.state];
  const rootClass = ["subagent-observation-card", className].filter(Boolean).join(" ");

  return (
    <section
      className={rootClass}
      data-state={snapshot.state}
      aria-label={stateLabel}
    >
      <header className="subagent-observation-card__head">
        <span className="subagent-observation-card__dot" aria-hidden />
        <span className="subagent-observation-card__state">{stateLabel}</span>
        <span className="subagent-observation-card__chips">
          {active && snapshot.stepCount > 0 ? (
            <span className="subagent-observation-card__chip">
              {labels.steps(snapshot.stepCount)}
            </span>
          ) : null}
          {!active ? (
            <span className="subagent-observation-card__chip">
              {labels.tools(snapshot.toolCount)}
            </span>
          ) : null}
          {durationMs != null ? (
            <span className="subagent-observation-card__chip">
              {labels.duration(durationMs)}
            </span>
          ) : null}
        </span>
      </header>

      {active ? (
        <div className="subagent-observation-card__body">
          {snapshot.actionTitle ? (
            <div
              className="subagent-observation-card__action"
              title={snapshot.actionPath ?? snapshot.actionTitle}
            >
              <span className="subagent-observation-card__action-title">
                {snapshot.actionTitle}
              </span>
              {snapshot.actionStatus ? (
                <span className="subagent-observation-card__action-status">
                  {snapshot.actionStatus}
                </span>
              ) : null}
              {snapshot.actionPath ? (
                <span className="subagent-observation-card__path">
                  {displayPath(snapshot.actionPath)}
                </span>
              ) : null}
            </div>
          ) : (
            <div className="subagent-observation-card__muted">{labels.noActionYet}</div>
          )}
          {snapshot.state === "waiting_user" ? (
            <div className="subagent-observation-card__note">
              {snapshot.waitingReason ?? labels.waitingHint}
            </div>
          ) : null}
          {snapshot.thoughtText ? (
            <div className="subagent-observation-card__thought">{snapshot.thoughtText}</div>
          ) : null}
        </div>
      ) : (
        <div className="subagent-observation-card__body">
          {snapshot.summaryText ? (
            <p className="subagent-observation-card__summary">{snapshot.summaryText}</p>
          ) : (
            <div className="subagent-observation-card__muted">{labels.emptySummary}</div>
          )}
        </div>
      )}

      {onOpenWorker && labels.openWorker ? (
        <div className="subagent-observation-card__actions">
          <button
            type="button"
            className="pill-action subagent-observation-card__open"
            onClick={onOpenWorker}
          >
            {labels.openWorker}
          </button>
        </div>
      ) : null}
    </section>
  );
}
