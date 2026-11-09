/**
 * UsageSessions — 最近对话列表(Token 用量页 v2 · e 区块)。
 * 每行:上下文占用圆环、标题、轮数、token、折算费用、相对时间;左缘 3px agent 墨色条。
 * 行尾「详情」打开单会话按轮面板;整行可点时(主窗/贴边)点行打开该对话。
 * 支持按标题 / agent / 项目过滤(大小写不敏感)。
 */
import { useMemo, useState } from "react";
import { Search, ChartLine } from "lucide-react";
import type { SessionRow, UsageInsights } from "../../lib/usageInsights";
import { agentDisplayName, agentInkStyle } from "../../lib/agentInk";
import { useT } from "../../lib/i18n";
import { fmtTokens, fmtUsd, Seg, SectionHead } from "./primitives";

type SortKey = "last" | "total" | "usd" | "ctx";

const PREVIEW = 12;
const RING_R = 9;
const RING_C = 2 * Math.PI * RING_R;

/** 降序比较;null(usd / ctxPct 可能缺)永远排在最后。 */
function cmpNullsLast(b: number | null, a: number | null): number {
  if (b == null && a == null) return 0;
  if (b == null) return 1;
  if (a == null) return -1;
  return b - a;
}

function sortSessions(rows: readonly SessionRow[], key: SortKey): SessionRow[] {
  return [...rows].sort((a, b) => {
    if (key === "last") return b.last - a.last;
    if (key === "total") return b.total - a.total;
    if (key === "usd") return cmpNullsLast(b.usd, a.usd);
    return cmpNullsLast(b.ctxPct, a.ctxPct);
  });
}

/** 24px 圆环:hairline 轨道,弧线用 agent 墨色,占用 ≥80% 转 yellow,12 点起笔。null 只画轨道。 */
function CtxRing({ pct }: { pct: number | null }) {
  const clamped = pct == null ? 0 : Math.min(Math.max(pct, 0), 100);
  const arc = (clamped / 100) * RING_C;
  const stroke = pct != null && pct >= 80 ? "var(--yellow)" : "var(--agent-ink, var(--accent))";
  return (
    <span className="uv2-sessions__ring">
      <svg width="24" height="24" viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="12" cy="12" r={RING_R} fill="none" stroke="var(--hairline)" strokeWidth="3" />
        {arc > 0 && (
          <circle
            cx="12"
            cy="12"
            r={RING_R}
            fill="none"
            stroke={stroke}
            strokeWidth="3"
            strokeDasharray={`${arc.toFixed(1)} ${RING_C.toFixed(1)}`}
            transform="rotate(-90 12 12)"
          />
        )}
      </svg>
      <span className="uv2-sessions__pct">{pct == null ? "—" : `${Math.round(pct)}%`}</span>
    </span>
  );
}

/** 相对时间的 i18n 键 + 参数:<1min 刚刚;<1h n 分钟前;当天 n 小时前;昨天;否则 M/D。 */
type RelTime =
  | { key: "usageV2.sessions.relNow" }
  | { key: "usageV2.sessions.relMin"; n: number }
  | { key: "usageV2.sessions.relHour"; n: number }
  | { key: "usageV2.sessions.relYesterday" }
  | { key: "usageV2.sessions.relDate"; m: number; d: number };

function relTime(last: number, now: number): RelTime {
  const diff = now - last;
  if (diff < 60_000) return { key: "usageV2.sessions.relNow" };
  if (diff < 3_600_000) return { key: "usageV2.sessions.relMin", n: Math.floor(diff / 60_000) };
  const midnight = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((midnight(new Date(now)) - midnight(new Date(last))) / 86_400_000);
  if (days === 0) return { key: "usageV2.sessions.relHour", n: Math.max(1, Math.floor(diff / 3_600_000)) };
  if (days === 1) return { key: "usageV2.sessions.relYesterday" };
  const d = new Date(last);
  return { key: "usageV2.sessions.relDate", m: d.getMonth() + 1, d: d.getDate() };
}

function relText(rel: RelTime, t: ReturnType<typeof useT>): string {
  if (rel.key === "usageV2.sessions.relMin" || rel.key === "usageV2.sessions.relHour") return t(rel.key, { n: rel.n });
  if (rel.key === "usageV2.sessions.relDate") return t(rel.key, { m: rel.m, d: rel.d });
  return t(rel.key);
}

/** 一行对话;onOpenSession 存在时渲染为按钮,否则是普通 div。 */
function SessionLine({
  row,
  now,
  onOpenSession,
  onOpenDetail,
}: {
  row: SessionRow;
  now: number;
  onOpenSession?: (id: string) => void;
  onOpenDetail: (row: SessionRow) => void;
}) {
  const t = useT();
  const agent = agentDisplayName(row.agentId);
  const sub =
    row.project == null ? t("usageV2.sessions.sub", { agent }) : t("usageV2.sessions.subProject", { agent, project: row.project });
  const className = onOpenSession ? "uv2-sessions__row is-click" : "uv2-sessions__row";
  const style = agentInkStyle(row.agentId);
  const cells = (
    <>
      <CtxRing pct={row.ctxPct} />
      <span className="uv2-sessions__t">
        <span className="uv2-sessions__label">{row.label}</span>
        <span className="uv2-sessions__sub">{sub}</span>
      </span>
      <span className="num">{t("usageV2.sessions.turns", { turns: row.turns })}</span>
      <span className="num">{fmtTokens(row.total)}</span>
      <span className="num">{fmtUsd(row.usd)}</span>
      <span className="num faint">{relText(relTime(row.last, now), t)}</span>
      <button
        type="button"
        className="uv2-sessions__detail"
        title={t("usageV2.sessions.detailOpen")}
        aria-label={`${t("usageV2.sessions.detailOpen")} · ${row.label}`}
        onClick={(event) => {
          event.stopPropagation();
          onOpenDetail(row);
        }}
      >
        <ChartLine size={13} strokeWidth={1.5} aria-hidden="true" />
      </button>
    </>
  );
  return onOpenSession ? (
    <button type="button" className={className} style={style} onClick={() => onOpenSession(row.id)}>
      {cells}
    </button>
  ) : (
    <div className={className} style={style}>
      {cells}
    </div>
  );
}

export function UsageSessions({
  ins,
  onOpenSession,
  onOpenDetail,
}: {
  ins: UsageInsights;
  onOpenSession?: (id: string) => void;
  onOpenDetail: (row: SessionRow) => void;
}) {
  const t = useT();
  const [sort, setSort] = useState<SortKey>("last");
  const [expanded, setExpanded] = useState(false);
  const [query, setQuery] = useState("");
  const now = Date.now();
  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    const scoped = q
      ? ins.sessions.filter(
          (row) =>
            row.label.toLowerCase().includes(q) ||
            agentDisplayName(row.agentId).toLowerCase().includes(q) ||
            (row.project ?? "").toLowerCase().includes(q),
        )
      : ins.sessions;
    return sortSessions(scoped, sort);
  }, [ins.sessions, query, sort]);
  const visible = expanded ? rows : rows.slice(0, PREVIEW);

  if (rows.length === 0 && !query.trim()) {
    return (
      <section className="uv2-sec uv2-sessions">
        <SectionHead title={t("usageV2.sessions.title")} />
        <p className="uv2-sessions__empty">{t("usageV2.sessions.empty")}</p>
      </section>
    );
  }

  const options = [
    { value: "last" as const, label: t("usageV2.sessions.sort.last") },
    { value: "total" as const, label: t("usageV2.sessions.sort.total") },
    { value: "usd" as const, label: t("usageV2.sessions.sort.usd") },
    { value: "ctx" as const, label: t("usageV2.sessions.sort.ctx") },
  ];

  return (
    <section className="uv2-sec uv2-sessions">
      <SectionHead
        title={t("usageV2.sessions.title")}
        meta={t("usageV2.sessions.meta", { count: rows.length })}
        right={
          <>
            <label className="uv2-sessions__search">
              <Search size={12} strokeWidth={1.5} aria-hidden="true" />
              <input
                type="search"
                value={query}
                placeholder={t("usageV2.sessions.search")}
                title={t("usageV2.sessions.searchTitle")}
                aria-label={t("usageV2.sessions.search")}
                onChange={(event) => {
                  setQuery(event.target.value);
                  setExpanded(false);
                }}
              />
            </label>
            <Seg value={sort} onChange={setSort} options={options} label={t("usageV2.sessions.sortLabel")} />
          </>
        }
      />
      {rows.length === 0 && query.trim() ? (
        <p className="uv2-sessions__empty">{t("usageV2.sessions.searchEmpty", { q: query.trim() })}</p>
      ) : (
        visible.map((row) => (
          <SessionLine key={row.id} row={row} now={now} onOpenSession={onOpenSession} onOpenDetail={onOpenDetail} />
        ))
      )}
      {rows.length > PREVIEW && (
        <button type="button" className="uv2-sessions__more" onClick={() => setExpanded((v) => !v)}>
          {expanded ? t("usageV2.sessions.showLess") : t("usageV2.sessions.showAll", { count: rows.length })}
        </button>
      )}
    </section>
  );
}
