/**
 * UsageSessionDetail — one session's per-turn usage panel (Token usage page v2).
 *
 * Opens from a row in the sessions list. Data comes from the per-session turn
 * rows the ledger appends at every turn/complete (`usage/<id>.jsonl` via
 * usageLedger.readSessionRows) — no transcript reload. Shows the context
 * occupancy line (each point = one turn's live reading, compactions marked),
 * outcome summary, and the turn list (newest first).
 */

import { useEffect, useMemo, useState } from "react";
import { X } from "lucide-react";
import { useT } from "../../lib/i18n";
import { agentDisplayName, agentInkStyle } from "../../lib/agentInk";
import { CHAT_PROJECT_ID, listProjects, listSessions } from "../../lib/api";
import { readSessionRows } from "../../lib/usageLedger";
import {
  COMPOSITION_ORDER,
  fetchGrokComposition,
  scaleComposition,
  type BackfillComposition,
  type CompositionKey,
} from "../../lib/usageBackfill";
import { fmtTokens } from "./primitives";

/** One segment colour per composition key (uv2 palette, two-colour language kept). */
export const COMP_COLOR: Record<CompositionKey, string> = {
  system: "var(--uv2-derived)",
  toolDefs: "var(--uv2-tool)",
  inject: "var(--uv2-other)",
  user: "var(--panel-soft)",
  attach: "var(--uv2-wait)",
  assistant: "var(--uv2-output)",
  reasoning: "var(--uv2-think)",
  toolCalls: "var(--uv2-cost)",
  toolResults: "var(--uv2-cache-read)",
};

/** The conversation-composition block: stacked bar + per-category rows.
 * Data is grok-only today (its session store keeps the request material). */
function CompositionBlock({ sessionId, agentId }: { sessionId: string; agentId: string }) {
  const t = useT();
  const [comp, setComp] = useState<BackfillComposition | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "none">("loading");

  useEffect(() => {
    let alive = true;
    setState("loading");
    setComp(null);
    if (agentId !== "grok-build") {
      setState("none");
      return;
    }
    void (async () => {
      // Marionette's session row carries the native id the adapter reports.
      const projects = await listProjects().catch(() => []);
      const lists = await Promise.all([
        listSessions(CHAT_PROJECT_ID).catch(() => []),
        ...projects.map((project) => listSessions(project.id).catch(() => [])),
      ]);
      const native =
        lists.flat().find((session) => session.id === sessionId)?.nativeSessionIds?.[0] ?? null;
      if (!alive) return;
      if (!native) {
        setState("none");
        return;
      }
      const found = await fetchGrokComposition(native);
      if (!alive) return;
      if (found) {
        setComp(found);
        setState("ready");
      } else {
        setState("none");
      }
    })().catch(() => {
      if (alive) setState("none");
    });
    return () => {
      alive = false;
    };
  }, [sessionId, agentId]);

  if (state !== "ready" || !comp) {
    return state === "loading" ? null : (
      <p className="uv2-sdet__empty">{t("usageV2.comp.none")}</p>
    );
  }
  const scaled = scaleComposition(comp);
  const label = (key: CompositionKey) => t(`usageV2.comp.k.${key}` as const);
  return (
    <>
      <p className="uv2-sdet__capt">
        {t("usageV2.comp.title")}
        {scaled.calibrated && scaled.coverage != null && (
          <span className="uv2-sdet__comp-note">
            {t("usageV2.comp.calibrated", { pct: `${Math.round(scaled.coverage * 100)}%` })}
          </span>
        )}
      </p>
      <div className="uv2-comp__bar" role="img" aria-label={t("usageV2.comp.title")}>
        {scaled.parts
          .filter((part) => part.tokens > 0)
          .map((part) => (
            <i
              key={part.key}
              style={{ flexGrow: part.tokens, background: COMP_COLOR[part.key] }}
              title={`${label(part.key)} · ${fmtTokens(part.tokens)}`}
            />
          ))}
      </div>
      <div className="uv2-sdet__rows uv2-comp__rows">
        {COMPOSITION_ORDER.filter((key) => (comp[key] as number) > 0).map((key) => {
          const part = scaled.parts.find((p) => p.key === key)!;
          return (
            <div className="uv2-sdet__row uv2-comp__row" key={key}>
              <i className="uv2-key" style={{ background: COMP_COLOR[key] }} aria-hidden="true" />
              <span className="uv2-sdet__model">{label(key)}</span>
              <span className="num">{`${Math.round(part.share * 100)}%`}</span>
              <span className="num faint">{fmtTokens(part.tokens)}</span>
            </div>
          );
        })}
        <div className="uv2-comp__total">
          <span className="num">{fmtTokens(scaled.total)}</span>
          <span className="faint">{scaled.calibrated ? t("usageV2.comp.anchor") : t("usageV2.comp.estimateOnly")}</span>
        </div>
      </div>
    </>
  );
}

type TurnRow = {
  t: number;
  model: string | null;
  agent: string | null;
  in: number | null;
  out: number | null;
  cach: number | null;
  cachw: number | null;
  ctx: { used: number | null; size: number | null } | null;
  outcome: "ended" | "cancelled" | "failed" | string;
  comp: number;
  tools: Record<string, { n: number }> | null;
  cmds: Record<string, { n: number }> | null;
  edits: { ops: number; add: number; del: number } | null;
};

function parseRows(lines: readonly string[]): TurnRow[] {
  const rows: TurnRow[] = [];
  for (const line of lines) {
    try {
      const parsed = JSON.parse(line) as TurnRow;
      if (parsed && typeof parsed.t === "number") rows.push(parsed);
    } catch {
      /* skip a torn tail line */
    }
  }
  return rows;
}

function rowTokens(row: TurnRow): number {
  return (row.in ?? 0) + (row.out ?? 0) + (row.cach ?? 0) + (row.cachw ?? 0);
}

function toolCount(row: TurnRow): number {
  if (!row.tools) return 0;
  return Object.values(row.tools).reduce((sum, t) => sum + (t?.n ?? 0), 0);
}

/** Context occupancy line: points = turn-end readings, ticks = compactions, dots = failed turns. */
function CtxLine({ rows }: { rows: readonly TurnRow[] }) {
  const t = useT();
  const points = rows.filter((row) => row.ctx && row.ctx.used != null && row.ctx.size != null && row.ctx.size > 0);
  if (points.length < 2) return <p className="uv2-sdet__empty">{t("usageV2.detail.ctxNone")}</p>;
  const w = 100;
  const h = 40;
  const maxPct = Math.max(...points.map((row) => (row.ctx!.used! / row.ctx!.size!) * 100));
  const top = Math.max(20, Math.ceil(maxPct / 20) * 20);
  const coords = points.map((row, i) => {
    const x = (i / (points.length - 1)) * w;
    const y = h - Math.min(1, (row.ctx!.used! / row.ctx!.size!) * 100 / top) * h;
    return { x, y, row };
  });
  const path = coords.map((p, i) => `${i === 0 ? "M" : "L"}${p.x.toFixed(2)},${p.y.toFixed(2)}`).join(" ");
  return (
    <svg
      className="uv2-sdet__ctx"
      viewBox={`0 0 ${w} ${h}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={t("usageV2.detail.ctxSeries")}
    >
      <line x1="0" y1={h * 0.8} x2={w} y2={h * 0.8} className="uv2-sdet__ctx-hot" />
      {coords.map((p, i) =>
        p.row.comp > 0 ? <line key={`c${i}`} x1={p.x} y1="0" x2={p.x} y2={h} className="uv2-sdet__ctx-comp" /> : null,
      )}
      <path d={path} className="uv2-sdet__ctx-line" />
      {coords.map((p, i) =>
        p.row.outcome === "failed" ? (
          <circle key={`f${i}`} cx={p.x} cy={p.y} r="1.4" className="uv2-sdet__ctx-fail" />
        ) : (
          <circle key={`o${i}`} cx={p.x} cy={p.y} r="1" className="uv2-sdet__ctx-dot" />
        ),
      )}
    </svg>
  );
}

function outcomeLabel(outcome: string, t: ReturnType<typeof useT>): string {
  if (outcome === "cancelled") return t("usageV2.detail.outcome.cancelled");
  if (outcome === "failed") return t("usageV2.detail.outcome.failed");
  return t("usageV2.detail.outcome.ended");
}

export function UsageSessionDetail({
  sessionId,
  label,
  agentId,
  onClose,
}: {
  sessionId: string;
  label: string;
  agentId: string;
  onClose: () => void;
}) {
  const t = useT();
  const [rows, setRows] = useState<TurnRow[] | null>(null);

  useEffect(() => {
    let alive = true;
    setRows(null);
    void readSessionRows(sessionId)
      .then((lines) => {
        if (alive) setRows(parseRows(lines));
      })
      .catch(() => {
        if (alive) setRows([]);
      });
    return () => {
      alive = false;
    };
  }, [sessionId]);

  const ordered = useMemo(() => (rows ? [...rows].sort((a, b) => b.t - a.t) : null), [rows]);
  const stats = useMemo(() => {
    if (!rows) return null;
    let compactions = 0;
    let fails = 0;
    let cancels = 0;
    let tokens = 0;
    for (const row of rows) {
      compactions += row.comp ?? 0;
      if (row.outcome === "failed") fails += 1;
      if (row.outcome === "cancelled") cancels += 1;
      tokens += rowTokens(row);
    }
    return { compactions, fails, cancels, tokens };
  }, [rows]);

  return (
    <section className="uv2-sec uv2-sdet" style={agentInkStyle(agentId)}>
      <div className="uv2-sdet__head">
        <span className="uv2-sdet__label" title={label}>{label}</span>
        <span className="uv2-sdet__sub">{agentDisplayName(agentId)}</span>
        <button type="button" className="uv2-ghost" onClick={onClose} aria-label={t("usageV2.sessions.detailOpen")}>
          <X size={13} strokeWidth={1.5} aria-hidden="true" />
        </button>
      </div>
      {rows == null || stats == null ? (
        <p className="uv2-sdet__empty">…</p>
      ) : rows.length === 0 ? (
        <p className="uv2-sdet__empty">{t("usageV2.detail.empty")}</p>
      ) : (
        <>
          <div className="uv2-sdet__stats">
            <div>
              <b>{rows.length.toLocaleString()}</b>
              <span>{t("usageV2.sessions.turns", { turns: rows.length })}</span>
            </div>
            <div>
              <b>{fmtTokens(stats.tokens)}</b>
              <span>token</span>
            </div>
            {stats.compactions > 0 && (
              <div>
                <b>{stats.compactions.toLocaleString()}</b>
                <span>{t("usageV2.detail.compactions", { n: stats.compactions })}</span>
              </div>
            )}
            {(stats.fails > 0 || stats.cancels > 0) && (
              <div>
                <b>{stats.fails} · {stats.cancels}</b>
                <span>{t("usageV2.detail.fails", { n: stats.fails, m: stats.cancels })}</span>
              </div>
            )}
          </div>
          <p className="uv2-sdet__capt">{t("usageV2.detail.ctxSeries")}</p>
          <CtxLine rows={rows} />
          <CompositionBlock sessionId={sessionId} agentId={agentId} />
          <div className="uv2-sdet__rows">
            {ordered!.slice(0, 200).map((row, i) => (
              <div className={`uv2-sdet__row is-${row.outcome}`} key={`${row.t}-${i}`}>
                <span className="num faint">
                  {new Date(row.t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                </span>
                <span className="uv2-sdet__model" title={row.model ?? undefined}>{row.model ?? "—"}</span>
                <span className="num">{fmtTokens(rowTokens(row))}</span>
                <span className="num faint">
                  {row.ctx && row.ctx.used != null && row.ctx.size != null && row.ctx.size > 0
                    ? `${Math.round((row.ctx.used / row.ctx.size) * 100)}%`
                    : "—"}
                </span>
                {toolCount(row) > 0 && (
                  <span className="num faint">{t("usageV2.detail.tools", { n: toolCount(row) })}</span>
                )}
                {row.comp > 0 && <span className="uv2-sdet__comp">⤾ {row.comp}</span>}
                <span className={`uv2-sdet__out is-${row.outcome}`}>{outcomeLabel(row.outcome, t)}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  );
}
