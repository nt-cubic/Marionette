import { useMemo, useState } from "react";
import { Check, ChevronDown, ChevronRight, Plus, RefreshCw, Search } from "lucide-react";
import { addProject, discoverRecentProjects, refreshExternalSessions } from "../lib/api";
import { useT } from "../lib/i18n";
import type { AgentConfig, DiscoveredProject, Project, ProjectDiscoveryReport } from "../lib/types";

/**
 * Recent-project discovery (T3 P1-6).
 *
 * Read-only: scans each agent's own session store and reports the project roots
 * it has already seen, with a session count. Rules:
 * - nothing is added without an explicit per-row "Add" click;
 * - no Git URL cloning, no remote reads;
 * - a failed scan is just "nothing found" and never blocks the normal
 *   Add Project path;
 * - an existing project is never added twice, and on-disk projects /
 *   preferences are never overwritten.
 *
 * Collapsed by default: the scan starts only when the user opens it, so the
 * first-run guide cannot slow startup.
 */

const normRoot = (raw: string) =>
  raw.trim().replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();

function stampMs(raw?: string): number {
  if (!raw) return 0;
  const n = Number(raw);
  if (Number.isFinite(n) && n > 1_000_000) return n;
  const parsed = Date.parse(raw);
  return Number.isFinite(parsed) ? parsed : 0;
}

export function RecentProjectDiscovery({
  existingProjects,
  agents,
  onProjectAdded,
  compact = false,
  defaultOpen = false,
}: {
  /** Projects already in the shelf (compared by rootPath). */
  existingProjects?: Project[];
  /** Used only to turn agent ids into readable labels. */
  agents?: AgentConfig[];
  /** Called after a project was added (parent refreshes the shelf / opens it). */
  onProjectAdded?: (project: Project) => void;
  /** Narrow sidebar variant. */
  compact?: boolean;
  defaultOpen?: boolean;
}) {
  const t = useT();
  const [open, setOpen] = useState(defaultOpen);
  const [loading, setLoading] = useState(false);
  const [report, setReport] = useState<ProjectDiscoveryReport | null>(null);
  const [error, setError] = useState("");
  const [adding, setAdding] = useState<string | null>(null);
  const [added, setAdded] = useState<string[]>([]);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});

  const existing = useMemo(() => {
    const set = new Set((existingProjects ?? []).map(project => normRoot(project.rootPath)));
    for (const root of added) set.add(root);
    return set;
  }, [existingProjects, added]);

  // A candidate below an existing project is already covered by that project
  // (the per-project import matches its subtree), so it is not offered again.
  const isCovered = (key: string): boolean => {
    if (existing.has(key)) return true;
    for (const root of existing) {
      if (root && key.startsWith(`${root}\\`)) return true;
    }
    return false;
  };

  const agentLabel = useMemo(() => {
    const map = new Map((agents ?? []).map(agent => [agent.id, agent.label]));
    return (id: string) => map.get(id) ?? id;
  }, [agents]);

  const scan = async () => {
    setLoading(true);
    setError("");
    try {
      const result = await discoverRecentProjects();
      setReport(result);
      if (result.failedSources.length > 0) {
        setError(t("discover.sourceFailed", { sources: result.failedSources.join(", ") }));
      }
    } catch {
      // The api wrapper already fails soft; this is the last guard so that
      // discovery can never surface as a blocking error.
      setReport({ projects: [], scannedSources: [], failedSources: [], truncated: false });
      setError(t("discover.failed"));
    } finally {
      setLoading(false);
    }
  };

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next && !report && !loading) void scan();
  };

  const addOne = async (candidate: DiscoveredProject) => {
    if (adding) return;
    const key = normRoot(candidate.rootPath);
    setAdding(candidate.rootPath);
    setRowErrors(current => {
      if (!(candidate.rootPath in current)) return current;
      const next = { ...current };
      delete next[candidate.rootPath];
      return next;
    });
    try {
      const project = await addProject(candidate.rootPath);
      // Reuse the existing per-project import; its failure must not undo the
      // fact that the project itself was added.
      await refreshExternalSessions(project.id).catch(() => undefined);
      setAdded(current => (current.includes(key) ? current : [...current, key]));
      onProjectAdded?.(project);
    } catch (failure) {
      setRowErrors(current => ({
        ...current,
        [candidate.rootPath]: t("discover.addFailed", {
          error: failure instanceof Error ? failure.message : String(failure),
        }),
      }));
    } finally {
      setAdding(null);
    }
  };

  const dayLabel = (raw?: string): string => {
    const ms = stampMs(raw);
    if (!ms) return "";
    const days = Math.floor((Date.now() - ms) / 86_400_000);
    if (days <= 0) return t("discover.today");
    if (days === 1) return t("discover.yesterday");
    if (days < 7) return t("discover.daysAgo", { count: days });
    return new Date(ms).toLocaleDateString();
  };

  const rows = report?.projects ?? [];

  return (
    <section className={`recent-projects${compact ? " is-compact" : ""}`} aria-label={t("discover.title")}>
      <button
        type="button"
        className="recent-projects__toggle"
        aria-expanded={open}
        onClick={toggle}
      >
        <Search size={12} aria-hidden />
        <span>{t("discover.title")}</span>
        {open ? <ChevronDown size={12} aria-hidden /> : <ChevronRight size={12} aria-hidden />}
      </button>
      {open && (
        <div className="recent-projects__body">
          <p className="recent-projects__hint">{t("discover.hint")}</p>
          <div className="recent-projects__bar">
            <button type="button" className="recent-projects__scan" disabled={loading} onClick={() => void scan()}>
              <RefreshCw size={12} aria-hidden className={loading ? "is-spinning" : undefined} />
              <span>{loading ? t("discover.scanning") : t("discover.rescan")}</span>
            </button>
          </div>
          {error && <p className="recent-projects__error">{error}</p>}
          {report && rows.length === 0 && !loading && (
            <p className="recent-projects__empty">{t("discover.empty")}</p>
          )}
          {rows.length > 0 && (
            <ul className="recent-projects__list">
              {rows.map(candidate => {
                const key = normRoot(candidate.rootPath);
                const exactlyAdded = existing.has(key);
                const isExisting = isCovered(key);
                const isAdding = adding === candidate.rootPath;
                const meta = [
                  t("discover.sessions", { count: candidate.sessionCount }),
                  candidate.agents.map(agentLabel).join(" / "),
                  dayLabel(candidate.lastActiveAt),
                ].filter(Boolean).join(" / ");
                return (
                  <li className="recent-projects__row" key={candidate.rootPath}>
                    <div className="recent-projects__main">
                      <b>{candidate.name}</b>
                      <span className="recent-projects__path" title={candidate.rootPath}>{candidate.rootPath}</span>
                      <span className="recent-projects__meta">{meta}</span>
                      {rowErrors[candidate.rootPath] && (
                        <span className="recent-projects__row-error">{rowErrors[candidate.rootPath]}</span>
                      )}
                    </div>
                    {isExisting ? (
                      <span className="recent-projects__done">
                        <Check size={12} aria-hidden />
                        {exactlyAdded ? t("discover.added") : t("discover.covered")}
                      </span>
                    ) : (
                      <button
                        type="button"
                        className="recent-projects__add"
                        disabled={adding !== null}
                        onClick={() => void addOne(candidate)}
                      >
                        <Plus size={12} aria-hidden />
                        {isAdding ? t("discover.adding") : t("discover.add")}
                      </button>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {report?.truncated && (
            <p className="recent-projects__truncated">{t("discover.truncated", { count: rows.length })}</p>
          )}
        </div>
      )}
    </section>
  );
}
