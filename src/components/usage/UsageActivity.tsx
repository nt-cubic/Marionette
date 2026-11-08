/**
 * Token usage page v2 · activity rankings (task D): tool kinds, command
 * programs and skills / MCP tools, each a ranked list with a share bar, plus
 * the file-edits footer on the skills band. Data comes from usageInsights.
 */
import { useState, type CSSProperties, type ReactNode } from "react";
import {
  Bot,
  FilePlus2,
  FileText,
  Globe,
  PencilLine,
  Plug,
  Search,
  Sparkles,
  Terminal,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import { useT, type DictKey } from "../../lib/i18n";
import { fmtMs, SectionHead } from "./primitives";
import type { RankRow, UsageInsights } from "../../lib/usageInsights";

/** Tool kind key → icon; keys outside the known set fall back to "other". */
const TOOL_KIND_ICONS: Record<string, LucideIcon> = {
  read: FileText,
  edit: PencilLine,
  write: FilePlus2,
  command: Terminal,
  search: Search,
  web: Globe,
  agent: Bot,
  other: Wrench,
};

const TOP_ROWS = 8;

function ToolLabel({ row }: { row: RankRow }) {
  const t = useT();
  const kind = TOOL_KIND_ICONS[row.key] ? row.key : "other";
  const Icon = TOOL_KIND_ICONS[kind]!;
  return (
    <>
      <Icon size={13} strokeWidth={1.5} aria-hidden="true" />
      <span className="uv2-act__label">{t(`usageV2.activity.kind.${kind}` as DictKey)}</span>
    </>
  );
}

function CmdLabel({ row }: { row: RankRow }) {
  return (
    <>
      <Terminal size={13} strokeWidth={1.5} aria-hidden="true" />
      <span className="uv2-act__label uv2-act__label--mono">{row.key}</span>
    </>
  );
}

function SkillLabel({ row }: { row: RankRow }) {
  const Icon = row.kind === "mcp" ? Plug : Sparkles;
  return (
    <>
      <Icon size={13} strokeWidth={1.5} aria-hidden="true" />
      <span className="uv2-act__label">{row.label}</span>
    </>
  );
}

/** One ranked list: top 8 rows, then a text toggle for the rest. */
function RankList({ rows, labelOf }: { rows: readonly RankRow[]; labelOf: (row: RankRow) => ReactNode }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  if (rows.length === 0) return <p className="uv2-act__empty">{t("usageV2.activity.none")}</p>;
  const max = rows[0]?.n ?? 0; // rows arrive sorted by n desc
  const shown = open ? rows : rows.slice(0, TOP_ROWS);
  const hidden = rows.length - shown.length;
  return (
    <div>
      {shown.map((row) => (
        <div className="uv2-act__row" key={row.key}>
          <span className="uv2-act__name">{labelOf(row)}</span>
          <span className="uv2-act__n num">{t("usageV2.activity.times", { n: row.n })}</span>
          <i className="uv2-act__bar" aria-hidden="true">
            <s style={{ width: `${max > 0 ? (row.n / max) * 100 : 0}%` }} />
          </i>
          <span className="uv2-act__x">
            {t("usageV2.activity.avg", { ms: fmtMs(row.avgMs) })}
            {row.fail > 0 && <b className="uv2-act__fail">{t("usageV2.activity.fail", { n: row.fail })}</b>}
          </span>
        </div>
      ))}
      {hidden > 0 && (
        <button type="button" className="uv2-act__more" aria-expanded={open} onClick={() => setOpen(!open)}>
          {open ? t("usageV2.activity.collapse") : t("usageV2.activity.more", { count: hidden })}
        </button>
      )}
    </div>
  );
}

export function UsageActivity({ ins }: { ins: UsageInsights }) {
  const t = useT();
  return (
    <div className="uv2-cols uv2-act" style={{ "--uv2-cols": 3 } as CSSProperties}>
      <section className="uv2-sec">
        <SectionHead
          title={t("usageV2.activity.tools")}
          meta={t("usageV2.activity.metaCalls", {
            n: ins.toolTotals.n.toLocaleString(),
            fail: ins.toolTotals.fail.toLocaleString(),
          })}
        />
        <RankList rows={ins.tools} labelOf={(row) => <ToolLabel row={row} />} />
      </section>
      <section className="uv2-sec">
        <SectionHead
          title={t("usageV2.activity.cmds")}
          meta={t("usageV2.activity.metaCalls", {
            n: ins.cmdTotals.n.toLocaleString(),
            fail: ins.cmdTotals.fail.toLocaleString(),
          })}
        />
        <RankList rows={ins.cmds} labelOf={(row) => <CmdLabel row={row} />} />
      </section>
      <section className="uv2-sec">
        <SectionHead
          title={t("usageV2.activity.skills")}
          meta={t("usageV2.activity.metaCount", { n: ins.skillTotals.n.toLocaleString() })}
        />
        <RankList rows={ins.skills} labelOf={(row) => <SkillLabel row={row} />} />
        <div className="uv2-act__files">
          <span>
            <b>{ins.edits.ops.toLocaleString()}</b>
            {t("usageV2.activity.edits")}
          </span>
          <span className="uv2-act__add">{t("usageV2.activity.added", { add: ins.edits.add.toLocaleString() })}</span>
          <span className="uv2-act__del">{t("usageV2.activity.deleted", { del: ins.edits.del.toLocaleString() })}</span>
        </div>
      </section>
    </div>
  );
}
