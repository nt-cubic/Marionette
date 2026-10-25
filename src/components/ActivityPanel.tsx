import { useEffect, useState, type ReactNode } from "react";
import {
  activitySpanWidthPct,
  activityTimeline,
  clipTeaser,
  formatSpanDuration,
  oneLine,
  tallyWord,
  toolKindVerb,
  type ActivitySummary,
  type ActivityTimelineData,
  type ActivityDetailItem,
} from "../lib/turnActivity";
import { ClippedBody } from "./ClippedBody";
import { MarkdownBody } from "./MarkdownBody";

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
      {summary.failed ? <span className="activity-panel__failed">{summary.failed} failed</span> : null}
      {liveIcon}
    </summary>
  );
}

type FoldProps = {
  summary: ActivitySummary;
  open: boolean;
  live?: boolean;
  liveIcon?: ReactNode;
  onToggle: (open: boolean) => void;
  children: ReactNode;
  trace?: ReactNode;
};

/**
 * `<details open>` writes the attribute on every state flip, and that write
 * fires `toggle` again. Only a real click can leave the DOM disagreeing with
 * the `open` prop, so anything else is our own controlled sync.
 */
function userToggled(details: HTMLDetailsElement, open: boolean): boolean {
  return details.open !== open;
}

/**
 * Codex / Claude-Code style activity fold: one summary row, nested folds inside.
 * Body mounts only while open so a 400-tool turn stays cheap until expanded.
 *
 * The live chart is outside the detail fold, so the details can stay closed
 * while time bars update. Ignore controlled attribute changes in onToggle.
 */
export function ActivityPanel({
  summary,
  open,
  live = false,
  liveIcon,
  onToggle,
  children,
  trace,
}: FoldProps) {
  return (
    <section className={`activity-group${live ? " is-live" : ""}`} aria-label="运行轨迹">
      {trace}
      <details
        className={`activity-panel${live ? " is-live" : ""}`}
        open={open}
        onToggle={(e) => {
          const details = e.currentTarget as HTMLDetailsElement;
          if (!userToggled(details, open)) return;
          onToggle(details.open);
        }}
      >
        <FoldSummary summary={summary} liveIcon={live ? liveIcon : null} />
        {open ? <div className="activity-panel__body">{children}</div> : null}
      </details>
    </section>
  );
}

/** Own the clock here: growing a live bar must not re-render the transcript. */
export function LiveActivityTimeline({ items }: { items: ActivityDetailItem[] }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, []);
  const data = activityTimeline(items, now);
  if (!data) return null;
  return (
    <div className="activity-trace" role="group" aria-label="实时轨迹面板">
      <div className="activity-trace__heading">轨迹面板</div>
      <ActivityTimeline data={data} />
    </div>
  );
}

/** A stable preview, independent of the detail fold and thought/tool switches. */
export function LiveThoughtPreview({
  items,
  thinking,
}: {
  items: ActivityDetailItem[];
  thinking: boolean;
}) {
  // Keep the previous text until the next thought has actual content. Empty
  // opening chunks must not blank the preview between two short thoughts.
  let text = "";
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const event = items[i]!.event;
    if (event.type === "thought" && event.text.trim()) {
      text = event.text;
      break;
    }
  }
  if (!text) return null;
  return (
    <div className="activity-thought" role="group" aria-label="当前思考预览">
      <div className="activity-thought__heading">{thinking ? "正在思考" : "最近思考"}</div>
      <ClippedBody className="activity-thought__body" maxHeight={72} stickToBottom>
        <MarkdownBody text={text} />
      </ClippedBody>
    </div>
  );
}

/** Same-kind run inside an activity panel ("Ran 3 commands"). */
export function ActivityKindGroup({
  summary,
  open,
  live = false,
  liveIcon,
  onToggle,
  children,
}: FoldProps) {
  return (
    <details
      className={`activity-kind${live ? " is-live" : ""}`}
      open={open}
      onToggle={(e) => {
        const details = e.currentTarget as HTMLDetailsElement;
        if (!userToggled(details, open)) return;
        onToggle(details.open);
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
    <div className="activity-timeline" role="img" aria-label={`运行时间轨迹，${formatSpanDuration(data.durationMs)}`}>
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
                const widthPct = activitySpanWidthPct(span, data.durationMs);
                return (
                  <i
                    key={i}
                    className={`activity-timeline__span is-${bucket === "thought" ? "thought" : "tool"}${
                      span.failed ? " is-failed" : ""
                    }${span.open ? " is-open" : ""}`}
                    style={{
                      left: `min(${pct(span.start)}, ${100 - widthPct}%)`,
                      width: `${widthPct}%`,
                    }}
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
