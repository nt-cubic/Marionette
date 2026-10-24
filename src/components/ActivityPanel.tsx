import type { ReactNode } from "react";
import {
  clipTeaser,
  formatSpanDuration,
  oneLine,
  tallyWord,
  toolKindVerb,
  type ActivitySummary,
  type ActivityTimelineData,
} from "../lib/turnActivity";

function DiffBadge({ diff }: { diff: { add: number; del: number } }) {
  return (
    <span className="event-card__tool-diff">
      <span className="event-card__tool-diff-add">+{diff.add}</span>
      <span className="event-card__tool-diff-del">−{diff.del}</span>
    </span>
  );
}

function FoldSummary({
  summary,
  liveIcon,
}: {
  summary: ActivitySummary;
  liveIcon?: ReactNode;
}) {
  return (
    <summary className="activity-panel__summary" title={summary.title}>
      <span className="activity-panel__title">{clipTeaser(summary.title, 96)}</span>
      {summary.preview ? (
        <span className="activity-panel__preview-wrap">
          <span className="activity-panel__preview">{summary.preview}</span>
        </span>
      ) : null}
      {summary.elapsed ? (
        <span className="activity-panel__elapsed">{summary.elapsed}</span>
      ) : null}
      {summary.diff ? <DiffBadge diff={summary.diff} /> : null}
      {liveIcon}
    </summary>
  );
}

type FoldProps = {
  id: string;
  summary: ActivitySummary;
  open: boolean;
  forceOpen?: boolean;
  live?: boolean;
  liveIcon?: ReactNode;
  onToggle: (open: boolean) => void;
  children: ReactNode;
};

/**
 * Codex / Claude-Code style activity fold: one summary row, nested folds inside.
 * Body mounts only while open so a 400-tool turn stays cheap until expanded.
 */
export function ActivityPanel({
  id,
  summary,
  open,
  forceOpen = false,
  live = false,
  liveIcon,
  onToggle,
  children,
}: FoldProps) {
  return (
    <details
      className={`activity-panel${live ? " is-live" : ""}`}
      key={`${id}${forceOpen ? "-live" : "-settled"}`}
      open={open}
      onToggle={(e) => {
        if (forceOpen) return;
        onToggle((e.currentTarget as HTMLDetailsElement).open);
      }}
    >
      <FoldSummary summary={summary} liveIcon={live ? liveIcon : null} />
      {open ? <div className="activity-panel__body">{children}</div> : null}
    </details>
  );
}

/** Same-kind run inside an activity panel ("Ran 3 commands"). */
export function ActivityKindGroup({
  id,
  summary,
  open,
  forceOpen = false,
  live = false,
  liveIcon,
  onToggle,
  children,
}: FoldProps) {
  return (
    <details
      className={`activity-kind${live ? " is-live" : ""}`}
      key={`${id}${forceOpen ? "-live" : "-settled"}`}
      open={open}
      onToggle={(e) => {
        if (forceOpen) return;
        onToggle((e.currentTarget as HTMLDetailsElement).open);
      }}
    >
      <FoldSummary summary={summary} liveIcon={live ? liveIcon : null} />
      {open ? <div className="activity-kind__body">{children}</div> : null}
    </details>
  );
}

/**
 * Run timeline at the top of an open fold: one lane per kind (tally order),
 * one bar per step on a shared time axis. Native title tooltips keep it free
 * of state; the fold body below still holds the full cards.
 */
export function ActivityTimeline({ data }: { data: ActivityTimelineData }) {
  const total = Math.max(data.durationMs, 1);
  const pct = (ms: number) => `${(ms / total) * 100}%`;
  return (
    <div className="activity-timeline" aria-hidden>
      {data.lanes.map((bucket) => {
        const spans = data.spans.filter((span) => span.bucket === bucket);
        return (
          <div key={bucket} className="activity-timeline__lane">
            <span className="activity-timeline__label">{tallyWord(bucket)}</span>
            <span className="activity-timeline__track">
              {spans.map((span, i) => {
                const ev = span.event;
                const what =
                  ev.type === "tool_call"
                    ? `${bucket === "thought" ? "Thought" : toolKindVerb(bucket)} · ${clipTeaser(oneLine(ev.title || ev.path || ""), 60)}`
                    : clipTeaser(oneLine(ev.type === "thought" ? ev.text : ""), 60);
                return (
                  <i
                    key={i}
                    className={`activity-timeline__span is-${bucket === "thought" ? "thought" : "tool"}${
                      span.failed ? " is-failed" : ""
                    }${span.open ? " is-open" : ""}`}
                    style={{ left: pct(span.start), width: pct(span.end - span.start) }}
                    title={`${what} · ${formatSpanDuration(span.end - span.start)}`}
                  />
                );
              })}
            </span>
            <span className="activity-timeline__count">{spans.length}</span>
          </div>
        );
      })}
      <div className="activity-timeline__axis">
        <span>0s</span>
        <span>{formatSpanDuration(data.durationMs)}</span>
      </div>
    </div>
  );
}
