import type { ReactNode } from "react";
import { clipTeaser, type ActivitySummary } from "../lib/turnActivity";

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
