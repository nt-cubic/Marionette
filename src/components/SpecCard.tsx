import { Check, Circle, Loader2, X } from "lucide-react";
import { Component, memo, useMemo, type ReactNode } from "react";
import {
  parseSpecSpec,
  SPEC_ACTION_EVENT,
  type SpecAction,
  type SpecCell,
  type SpecPoint,
  type SpecSpec,
  type SpecState,
} from "../lib/specSchema";
import { t, useT } from "../lib/i18n";

/**
 * ` ```spec ` fence → interactive card (generative UI, research-codeg.md B4).
 *
 * The fence body is parsed by specSchema's three guards first; this component
 * is guard four: a React error boundary so a render-time failure (a shape the
 * validator passed but the view chokes on) degrades this one card instead of
 * the transcript. Failed/incomplete parses show the raw source quietly —
 * during streaming the JSON is half-arrived and the note reads as "waiting",
 * which is the truthful state; a genuinely malformed fence keeps the same
 * card so the user can see what the agent actually wrote.
 *
 * Action buttons never send anything themselves: they dispatch
 * SPEC_ACTION_EVENT and App.tsx routes the prompt into the composer
 * (prefill, never auto-send).
 */

function dispatchSpecAction(prompt: string): void {
  window.dispatchEvent(new CustomEvent<string>(SPEC_ACTION_EVENT, { detail: prompt }));
}

function SpecActions({ actions }: { actions: SpecAction[] }) {
  if (actions.length === 0) return null;
  return (
    <div className="md-spec__actions">
      {actions.map((action) => (
        <button
          key={`${action.label}\u0000${action.prompt}`}
          type="button"
          className="pill-action md-spec__action"
          onClick={() => dispatchSpecAction(action.prompt)}
        >
          {action.label}
        </button>
      ))}
    </div>
  );
}

function CardView({ spec }: { spec: Extract<SpecSpec, { type: "card" }> }) {
  return (
    <div className="md-spec__inner">
      {spec.title ? <div className="md-spec__title">{spec.title}</div> : null}
      <dl className="md-spec__fields">
        {spec.fields.map((field) => (
          <div key={`${field.name}\u0000${field.value}`} className="md-spec__field">
            <dt>{field.name}</dt>
            <dd>
              {field.value}
              {field.hint ? <span className="md-spec__field-hint">{field.hint}</span> : null}
            </dd>
          </div>
        ))}
      </dl>
      {spec.body ? <div className="md-spec__body">{spec.body}</div> : null}
      <SpecActions actions={spec.actions ?? []} />
    </div>
  );
}

function cellClass(cell: SpecCell): string {
  if (typeof cell === "string" || !cell.tone) return "md-spec__cell";
  return `md-spec__cell is-${cell.tone}`;
}

function TableView({ spec }: { spec: Extract<SpecSpec, { type: "table" }> }) {
  return (
    <div className="md-spec__inner">
      {spec.title ? <div className="md-spec__title">{spec.title}</div> : null}
      <div className="md-spec__table-wrap">
        <table className="md-spec__table">
          <thead>
            <tr>
              {spec.columns.map((column) => (
                <th key={column}>{column}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {spec.rows.map((row, rowIndex) => (
              <tr key={rowIndex}>
                {row.map((cell, colIndex) => (
                  <td key={colIndex} className={cellClass(cell)}>
                    {typeof cell === "string" ? cell : cell.text}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <SpecActions actions={spec.actions ?? []} />
    </div>
  );
}

const STATE_ICON: Record<SpecState, ReactNode> = {
  done: <Check size={12} />,
  active: <Loader2 size={12} className="md-spec__spin" />,
  todo: <Circle size={12} />,
  error: <X size={12} />,
};

function StepsView({ spec }: { spec: Extract<SpecSpec, { type: "steps" }> }) {
  return (
    <div className="md-spec__inner">
      {spec.title ? <div className="md-spec__title">{spec.title}</div> : null}
      <ol className="md-spec__steps">
        {spec.items.map((item, index) => (
          <li key={`${index}\u0000${item.label}`} className={`md-spec__step is-${item.state}`}>
            <span className="md-spec__step-icon">{STATE_ICON[item.state]}</span>
            <span className="md-spec__step-text">
              {item.label}
              {item.note ? <span className="md-spec__step-note">{item.note}</span> : null}
            </span>
          </li>
        ))}
      </ol>
      <SpecActions actions={spec.actions ?? []} />
    </div>
  );
}

/** Series colors cycle over CSS classes so the theme owns the palette. */
const SERIES_CLASSES = [0, 1, 2, 3, 4, 5, 6, 7].map((index) => `md-spec__series-${index}`);

function seriesClass(index: number): string {
  return SERIES_CLASSES[index % SERIES_CLASSES.length]!;
}

function pointLabel(point: SpecPoint, index: number): string {
  return point.label ?? `#${index + 1}`;
}

function BarChart({ series }: { series: { name?: string; points: SpecPoint[] }[] }) {
  const flat = series.flatMap((group, groupIndex) =>
    group.points.map((point, pointIndex) => ({
      label: pointLabel(point, pointIndex),
      value: point.value,
      seriesIndex: groupIndex,
      seriesName: group.name,
    })),
  );
  const max = Math.max(...flat.map((row) => Math.abs(row.value)), 1);
  return (
    <div className="md-spec__bars">
      {flat.map((row) => (
        <div key={`${row.label}\u0000${row.value}`} className="md-spec__bar-row">
          <span className="md-spec__bar-label" title={row.seriesName ? `${row.seriesName} · ${row.label}` : row.label}>
            {row.label}
          </span>
          <span className="md-spec__bar-track">
            <span
              className={`md-spec__bar-fill ${seriesClass(row.seriesIndex)}`}
              style={{ width: `${Math.max((Math.abs(row.value) / max) * 100, row.value === 0 ? 0 : 2)}%` }}
            />
          </span>
          <span className="md-spec__bar-value">{row.value}</span>
        </div>
      ))}
    </div>
  );
}

const LINE_W = 320;
const LINE_H = 140;
const LINE_PAD = { top: 10, right: 8, bottom: 18, left: 8 };

function LineChart({ series }: { series: { name?: string; points: SpecPoint[] }[] }) {
  const values = series.flatMap((group) => group.points.map((point) => point.value));
  const maxIndex = Math.max(...series.map((group) => group.points.length), 1);
  const min = Math.min(...values, 0);
  const max = Math.max(...values, 1);
  const span = max - min || 1;
  const x = (index: number) =>
    LINE_PAD.left + (maxIndex <= 1 ? 0 : (index / (maxIndex - 1)) * (LINE_W - LINE_PAD.left - LINE_PAD.right));
  const y = (value: number) =>
    LINE_PAD.top + (1 - (value - min) / span) * (LINE_H - LINE_PAD.top - LINE_PAD.bottom);
  return (
    <svg
      className="md-spec__line"
      viewBox={`0 0 ${LINE_W} ${LINE_H}`}
      role="img"
      aria-label={series.map((group) => group.name ?? "series").join(" / ")}
    >
      <line
        className="md-spec__axis"
        x1={LINE_PAD.left}
        x2={LINE_W - LINE_PAD.right}
        y1={y(Math.max(min, 0))}
        y2={y(Math.max(min, 0))}
      />
      {series.map((group, groupIndex) => (
        <g key={groupIndex} className={seriesClass(groupIndex)}>
          <polyline
            className="md-spec__line-path"
            points={group.points.map((point, index) => `${x(index)},${y(point.value)}`).join(" ")}
          />
          {group.points.map((point, index) => (
            <circle key={index} className="md-spec__line-dot" cx={x(index)} cy={y(point.value)} r={2.5}>
              <title>{`${pointLabel(point, index)}: ${point.value}`}</title>
            </circle>
          ))}
        </g>
      ))}
    </svg>
  );
}

function PieChart({ series }: { series: { name?: string; points: SpecPoint[] }[] }) {
  const flat = series.flatMap((group, groupIndex) =>
    group.points.map((point, pointIndex) => ({
      label: pointLabel(point, pointIndex),
      value: Math.max(point.value, 0),
      seriesIndex: groupIndex,
      seriesName: group.name,
    })),
  );
  const total = flat.reduce((sum, row) => sum + row.value, 0) || 1;
  const R = 38;
  const C = 2 * Math.PI * R;
  let offset = 0;
  const arcs = flat.map((row) => {
    const arc = { ...row, fraction: row.value / total, offset };
    offset += row.value / total;
    return arc;
  });
  return (
    <div className="md-spec__pie">
      <svg className="md-spec__pie-svg" viewBox="0 0 100 100" role="img" aria-label="pie chart">
        {arcs.map((arc, index) => (
          <circle
            key={index}
            className={`md-spec__pie-arc ${seriesClass(arc.seriesIndex)}`}
            cx="50"
            cy="50"
            r={R}
            fill="none"
            strokeWidth="14"
            strokeDasharray={`${arc.fraction * C} ${C}`}
            strokeDashoffset={-arc.offset * C}
          >
            <title>{`${arc.label}: ${arc.value} (${Math.round(arc.fraction * 100)}%)`}</title>
          </circle>
        ))}
      </svg>
      <ul className="md-spec__legend">
        {arcs.map((arc, index) => (
          <li key={index}>
            <span className={`md-spec__legend-dot ${seriesClass(arc.seriesIndex)}`} />
            <span className="md-spec__legend-label">{arc.seriesName ? `${arc.seriesName} · ` : ""}{arc.label}</span>
            <span className="md-spec__legend-value">
              {arc.value} · {Math.round(arc.fraction * 100)}%
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function ChartView({ spec }: { spec: Extract<SpecSpec, { type: "chart" }> }) {
  return (
    <div className="md-spec__inner">
      {spec.title ? <div className="md-spec__title">{spec.title}</div> : null}
      {spec.chart === "bar" ? <BarChart series={spec.series} /> : null}
      {spec.chart === "line" ? <LineChart series={spec.series} /> : null}
      {spec.chart === "pie" ? <PieChart series={spec.series} /> : null}
      <SpecActions actions={spec.actions ?? []} />
    </div>
  );
}

function SpecBody({ spec }: { spec: SpecSpec }) {
  switch (spec.type) {
    case "card":
      return <CardView spec={spec} />;
    case "table":
      return <TableView spec={spec} />;
    case "steps":
      return <StepsView spec={spec} />;
    case "chart":
      return <ChartView spec={spec} />;
  }
}

/** Guard four: a render crash degrades only this card. */
class SpecBoundary extends Component<{ raw: string; children: ReactNode }, { error: string | null }> {
  state = { error: null as string | null };

  static getDerivedStateFromError(error: unknown) {
    return { error: error instanceof Error ? error.message : String(error) };
  }

  render() {
    if (this.state.error !== null) {
      return <SpecFailure note={t("md.specRenderFailed", { error: this.state.error })} raw={this.props.raw} />;
    }
    return this.props.children;
  }
}

function SpecFailure({ note, raw }: { note: string; raw: string }) {
  return (
    <div className="md-spec md-spec--pending">
      <div className="md-spec__note">{note}</div>
      <pre className="md-spec__src">{raw}</pre>
    </div>
  );
}

export const SpecCard = memo(function SpecCard({ raw }: { raw: string }) {
  const t = useT();
  const parsed = useMemo(() => parseSpecSpec(raw.replace(/\n$/, "")), [raw]);
  if (!parsed.ok) {
    return <SpecFailure note={t("md.specIncomplete")} raw={raw} />;
  }
  return (
    <div className="md-spec">
      <SpecBoundary raw={raw}>
        <SpecBody spec={parsed.spec} />
      </SpecBoundary>
    </div>
  );
});
