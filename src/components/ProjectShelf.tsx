import { useEffect, useMemo, useRef, useState } from "react";
import { Bell, BellOff, ChevronDown, ChevronRight, Folder, Moon, PanelLeftClose, PanelLeftOpen, Pencil, Plus, RefreshCw, Search, Settings2, Sun, Trash2 } from "lucide-react";
import type { AgentConfig, ExternalConversation, Project, Session } from "../lib/types";

type ThemeMode = "dark" | "light";

/** Visible projects before "…" collapse (Codex-style). */
const PROJECT_LIST_PREVIEW = 5;
/** Visible sessions per project before "…" collapse. */
const SESSION_LIST_PREVIEW = 5;

function sessionRecency(session: Session): number {
  const raw = session.lastActiveAt || session.startedAt || "";
  const asNum = Number(raw);
  if (Number.isFinite(asNum) && asNum > 1e11) return asNum;
  const parsed = Date.parse(raw);
  if (Number.isFinite(parsed)) return parsed;
  // Fallback: session-… millis ids
  const idNum = Number(String(session.id).replace(/^session-/, ""));
  return Number.isFinite(idNum) ? idNum : 0;
}

function sortSessionsNewestFirst(list: Session[]): Session[] {
  return [...list].sort((a, b) => sessionRecency(b) - sessionRecency(a));
}

/** Active session first, then newest — matches “new dialog on top” + keep selection visible. */
function orderSessionsForShelf(list: Session[], currentSessionId?: string): Session[] {
  const sorted = sortSessionsNewestFirst(list);
  if (!currentSessionId) return sorted;
  const active = sorted.find((s) => s.id === currentSessionId);
  if (!active) return sorted;
  return [active, ...sorted.filter((s) => s.id !== currentSessionId)];
}

type ProjectShelfProps = {
  agents: AgentConfig[];
  projects: Project[];
  sessions: Session[];
  currentProjectId: string;
  currentSessionId?: string;
  collapsed: boolean;
  theme: ThemeMode;
  /** Session ids matched by transcript search (null = no active query). */
  searchHitIds?: string[] | null;
  onSearchQueryChange?: (query: string) => void;
  onCollapse: () => void;
  onExpand: () => void;
  onToggleTheme: () => void;
  /** Desktop attention (taskbar flash + sound) when AI replies / stalls. */
  desktopNotifyEnabled?: boolean;
  onToggleDesktopNotify?: () => void;
  onAddProject: () => void;
  onNewSession: (projectId: string) => void;
  onProjectSelect: (projectId: string) => void;
  onSessionSelect: (session: Session) => void;
  onDeleteSession: (sessionId: string) => void;
  onDeleteProject: (projectId: string) => void;
  /** Manual rename — always persists (unlike first-message auto-title). */
  onRenameSession?: (sessionId: string, label: string) => void;
  /** External agent sessions for the current project (memory-only cache). */
  externalSessions?: ExternalConversation[];
  externalScanning?: boolean;
  externalStatus?: string | null;
  currentExternalId?: string | null;
  onRefreshExternal?: (projectId: string) => void;
  onExternalSelect?: (conv: ExternalConversation) => void;
};

function externalSourceLabel(source: string): string {
  switch (source) {
    case "grok":
      return "Grok";
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "opencode":
      return "OpenCode";
    default:
      return source;
  }
}

function sessionIsBusy(status: Session["status"]): boolean {
  return status === "running" || status === "starting";
}

export function ProjectShelf({
  agents,
  projects,
  sessions,
  currentProjectId,
  currentSessionId,
  onProjectSelect,
  onSessionSelect,
  collapsed,
  onCollapse,
  onExpand,
  onAddProject,
  onNewSession,
  theme,
  onToggleTheme,
  desktopNotifyEnabled = true,
  onToggleDesktopNotify,
  onDeleteSession,
  onDeleteProject,
  onRenameSession,
  searchHitIds = null,
  onSearchQueryChange,
  externalSessions = [],
  externalScanning = false,
  externalStatus = null,
  currentExternalId = null,
  onRefreshExternal,
  onExternalSelect,
}: ProjectShelfProps) {
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(() => new Set(projects.map((project) => project.id)));
  const [query, setQuery] = useState("");
  const [projectsExpanded, setProjectsExpanded] = useState(false);
  /** Project ids whose session list is fully expanded past the preview cap. */
  const [sessionsExpandedByProject, setSessionsExpandedByProject] = useState<Set<string>>(() => new Set());
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!renamingId) return;
    renameInputRef.current?.focus();
    renameInputRef.current?.select();
  }, [renamingId]);

  const commitRename = () => {
    if (!renamingId || !onRenameSession) {
      setRenamingId(null);
      return;
    }
    const next = renameDraft.trim() || "New session";
    onRenameSession(renamingId, next);
    setRenamingId(null);
  };

  useEffect(() => {
    setExpandedProjects((current) => new Set([...current, ...projects.map((project) => project.id)]));
  }, [projects]);

  const agentLabel = (agentId: string) =>
    agents.find((a) => a.id === agentId)?.label ?? agentId;

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) {
      return { projects, sessionsByProject: null as Map<string, Session[]> | null };
    }
    const hitSet = searchHitIds ? new Set(searchHitIds) : null;
    const sessionsByProject = new Map<string, Session[]>();
    const projectIds = new Set<string>();

    for (const session of sessions) {
      const metaMatch =
        session.label.toLowerCase().includes(q) ||
        session.agentId.toLowerCase().includes(q) ||
        agentLabel(session.agentId).toLowerCase().includes(q);
      const transcriptMatch = hitSet?.has(session.id) ?? false;
      if (metaMatch || transcriptMatch) {
        projectIds.add(session.projectId);
        const list = sessionsByProject.get(session.projectId) ?? [];
        list.push(session);
        sessionsByProject.set(session.projectId, list);
      }
    }

    for (const project of projects) {
      if (project.name.toLowerCase().includes(q) || project.rootPath.toLowerCase().includes(q)) {
        projectIds.add(project.id);
        if (!sessionsByProject.has(project.id)) {
          sessionsByProject.set(
            project.id,
            sessions.filter((s) => s.projectId === project.id)
          );
        }
      }
    }

    return {
      projects: projects.filter((p) => projectIds.has(p.id)),
      sessionsByProject,
    };
  }, [agents, projects, query, searchHitIds, sessions]);

  // Auto-expand projects that have matches while searching.
  useEffect(() => {
    if (!query.trim()) return;
    setExpandedProjects(new Set(filtered.projects.map((p) => p.id)));
  }, [filtered.projects, query]);

  const toggleProject = (projectId: string) => {
    setExpandedProjects((current) => {
      const next = new Set(current);
      if (next.has(projectId)) next.delete(projectId);
      else next.add(projectId);
      return next;
    });
  };

  const themeButton = (
    <button
      className="sidebar-footer__button"
      type="button"
      title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
      aria-label={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
      onClick={onToggleTheme}
    >
      {theme === "dark" ? <Sun size={14} /> : <Moon size={14} />}
    </button>
  );

  const notifyButton = onToggleDesktopNotify ? (
    <button
      className={
        desktopNotifyEnabled
          ? "sidebar-footer__button is-notify-on"
          : "sidebar-footer__button is-notify-off"
      }
      type="button"
      title={
        desktopNotifyEnabled
          ? "Desktop notify on · taskbar flash + sound when AI replies or may be stuck"
          : "Desktop notify off · click to enable"
      }
      aria-label={desktopNotifyEnabled ? "Disable desktop notifications" : "Enable desktop notifications"}
      aria-pressed={desktopNotifyEnabled}
      onClick={onToggleDesktopNotify}
    >
      {desktopNotifyEnabled ? <Bell size={14} /> : <BellOff size={14} />}
    </button>
  ) : null;

  const allVisibleProjects = filtered.projects;
  const searching = Boolean(query.trim());
  // While searching, always show full match list; otherwise cap at preview count.
  // Always keep the active project in the preview so selection never "disappears".
  const shouldCollapseList = !searching && !projectsExpanded && allVisibleProjects.length > PROJECT_LIST_PREVIEW;
  let visibleProjects = allVisibleProjects;
  if (shouldCollapseList) {
    const head = allVisibleProjects.slice(0, PROJECT_LIST_PREVIEW);
    if (head.some((p) => p.id === currentProjectId)) {
      visibleProjects = head;
    } else {
      const active = allVisibleProjects.find((p) => p.id === currentProjectId);
      visibleProjects = active ? [...head.slice(0, PROJECT_LIST_PREVIEW - 1), active] : head;
    }
  }
  const hiddenCount = Math.max(0, allVisibleProjects.length - visibleProjects.length);

  return (
    <section className="project-tree">
      {/* Same height as center tabs / right Information — flush to window top like Zed. */}
      <div className="sidebar-title titlebar-row">
        {/* Full-bleed drag layer behind the field; input sits above so typing still works. */}
        <div className="titlebar-drag-layer" data-tauri-drag-region />
        <label className="sidebar-search sidebar-search--input sidebar-search--titlebar">
          <Search size={13} aria-hidden />
          <input
            type="search"
            value={query}
            placeholder="Search threads…"
            aria-label="Search threads"
            onChange={(event) => {
              const next = event.target.value;
              setQuery(next);
              onSearchQueryChange?.(next);
            }}
          />
        </label>
      </div>

      <div className="project-tree__body">
      <button className="project-add" type="button" title="Add project" onClick={onAddProject}>
        <Plus size={13} />
        Add project
      </button>

      <div className="project-list custom-scrollbar scrollbar-autohide" aria-label="Projects with sessions">
        {allVisibleProjects.length === 0 && (
          <div className="project-list__empty">
            {searching ? `No threads match “${query.trim()}”` : "No projects yet"}
          </div>
        )}
        {visibleProjects.map((project) => {
          const rawSessions =
            filtered.sessionsByProject?.get(project.id) ??
            sessions.filter((session) => session.projectId === project.id);
          // Active project: keep current session pinned first. Others: newest first.
          const projectSessions = orderSessionsForShelf(
            rawSessions,
            project.id === currentProjectId ? currentSessionId : undefined
          );
          const sessionsFullyExpanded = searching || sessionsExpandedByProject.has(project.id);
          const shouldCollapseSessions =
            !sessionsFullyExpanded && projectSessions.length > SESSION_LIST_PREVIEW;
          const visibleSessions = shouldCollapseSessions
            ? projectSessions.slice(0, SESSION_LIST_PREVIEW)
            : projectSessions;
          const hiddenSessionCount = Math.max(0, projectSessions.length - visibleSessions.length);

          return (
            <div className="project-group" key={project.id}>
              <div className={project.id === currentProjectId ? "project-row is-active" : "project-row"}>
                <button className="project-row__toggle" type="button" title={expandedProjects.has(project.id) ? "Collapse project" : "Expand project"} aria-label={expandedProjects.has(project.id) ? `Collapse ${project.name}` : `Expand ${project.name}`} aria-expanded={expandedProjects.has(project.id)} onClick={() => toggleProject(project.id)}>
                  {expandedProjects.has(project.id) ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                </button>
                <button className="project-row__select" type="button" title={project.rootPath} onClick={() => onProjectSelect(project.id)}>
                  <Folder size={15} />
                  <span className="project-row__content">
                    <strong>{project.name}</strong>
                  </span>
                </button>
                <span className="project-row__actions">
                  <button className="project-row__action" type="button" title="New session" onClick={() => onNewSession(project.id)}>
                    <Plus size={13} />
                  </button>
                  <button
                    className="project-row__action project-row__action--danger"
                    type="button"
                    title={`Remove project ${project.name}`}
                    aria-label={`Remove project ${project.name}`}
                    onClick={() => {
                      if (window.confirm(`Remove project “${project.name}” from the list?\n\nThis does not delete files on disk.`)) {
                        onDeleteProject(project.id);
                      }
                    }}
                  >
                    <Trash2 size={13} />
                  </button>
                  <button className="project-row__action" type="button" title="Project settings">
                    <Settings2 size={13} />
                  </button>
                </span>
              </div>

              {expandedProjects.has(project.id) && (
                <div className="project-sessions">
                  {projectSessions.length === 0 && (
                    <div className="session-row session-row--empty">No sessions</div>
                  )}
                  {visibleSessions.map((session) => {
                    const busy = sessionIsBusy(session.status);
                    const isRenaming = renamingId === session.id;
                    const beginRename = () => {
                      if (!onRenameSession) return;
                      setRenamingId(session.id);
                      setRenameDraft(session.label);
                    };
                    return (
                      <div
                        className={
                          !currentExternalId && session.id === currentSessionId
                            ? `session-row is-active is-${session.status}`
                            : `session-row is-${session.status}`
                        }
                        key={session.id}
                      >
                        {isRenaming ? (
                          <form
                            className="session-row__rename"
                            onSubmit={(e) => {
                              e.preventDefault();
                              commitRename();
                            }}
                          >
                            {busy ? (
                              <span
                                className={`session-row__pulse is-${session.status}`}
                                aria-hidden
                              />
                            ) : (
                              <span className="session-row__dot" aria-hidden />
                            )}
                            <input
                              ref={renameInputRef}
                              className="session-row__rename-input"
                              value={renameDraft}
                              aria-label="Rename session"
                              onChange={(e) => setRenameDraft(e.target.value)}
                              onBlur={() => commitRename()}
                              onKeyDown={(e) => {
                                if (e.key === "Escape") {
                                  e.preventDefault();
                                  setRenamingId(null);
                                }
                              }}
                            />
                          </form>
                        ) : (
                          <button
                            className="session-row__select"
                            type="button"
                            title={
                              busy
                                ? `${agentLabel(session.agentId)} · ${session.status} · double-click or pencil to rename`
                                : `${agentLabel(session.agentId)} · double-click or pencil to rename`
                            }
                            onClick={() => onSessionSelect(session)}
                            onDoubleClick={(e) => {
                              e.preventDefault();
                              e.stopPropagation();
                              beginRename();
                            }}
                          >
                            {busy ? (
                              <span
                                className={`session-row__pulse is-${session.status}`}
                                title={session.status}
                                aria-label={session.status}
                              />
                            ) : (
                              <span
                                className={`session-row__dot is-${session.status}`}
                                aria-hidden
                              />
                            )}
                            <span className="session-row__content">
                              <strong>{session.label}</strong>
                              <em>{agentLabel(session.agentId)}</em>
                            </span>
                          </button>
                        )}
                        <span className="session-row__actions">
                          {onRenameSession && !isRenaming && (
                            <button
                              className="session-row__action"
                              type="button"
                              title={`Rename ${session.label}`}
                              aria-label={`Rename ${session.label}`}
                              onClick={(e) => {
                                e.stopPropagation();
                                beginRename();
                              }}
                            >
                              <Pencil size={12} />
                            </button>
                          )}
                          <button
                            className="session-row__action session-row__action--danger"
                            type="button"
                            title={`Delete ${session.label}`}
                            aria-label={`Delete ${session.label}`}
                            onClick={() => onDeleteSession(session.id)}
                          >
                            <Trash2 size={12} />
                          </button>
                        </span>
                      </div>
                    );
                  })}

                  {project.id === currentProjectId && (externalSessions.length > 0 || onRefreshExternal) && (
                    <div className="external-sessions">
                      <div className="external-sessions__header">
                        <span className="external-sessions__title">
                          External{externalSessions.length > 0 ? ` (${externalSessions.length})` : ""}
                        </span>
                        {onRefreshExternal && (
                          <button
                            className="session-row__action"
                            type="button"
                            title="Refresh external sessions"
                            aria-label="Refresh external sessions"
                            disabled={externalScanning}
                            onClick={() => onRefreshExternal(project.id)}
                          >
                            <RefreshCw size={12} className={externalScanning ? "is-spinning" : undefined} />
                          </button>
                        )}
                      </div>
                      {externalStatus && (
                        <div className="external-sessions__status">{externalStatus}</div>
                      )}
                      {externalSessions.map((conv) => (
                        <div
                          key={conv.id}
                          className={
                            conv.id === currentExternalId
                              ? "session-row session-row--external is-active"
                              : "session-row session-row--external"
                          }
                        >
                          <button
                            className="session-row__select"
                            type="button"
                            title={`${externalSourceLabel(conv.source)} · read-only · ${conv.cwd}`}
                            onClick={() => onExternalSelect?.(conv)}
                          >
                            <span className="session-row__dot is-external" aria-hidden />
                            <span className="session-row__content">
                              <strong>{conv.title}</strong>
                              <em>[{externalSourceLabel(conv.source)}]</em>
                            </span>
                          </button>
                        </div>
                      ))}
                      {externalSessions.length === 0 && !externalScanning && (
                        <div className="session-row session-row--empty">
                          {externalStatus ?? "Click ↻ to scan"}
                        </div>
                      )}
                    </div>
                  )}

                  {shouldCollapseSessions && (
                    <div className="shelf-clip shelf-clip--sessions">
                      <div className="shelf-clip__fade" aria-hidden />
                      <button
                        className="shelf-clip__more"
                        type="button"
                        title={`Show ${hiddenSessionCount} more session${hiddenSessionCount === 1 ? "" : "s"}`}
                        aria-label={`Show ${hiddenSessionCount} more sessions`}
                        onClick={() => {
                          setSessionsExpandedByProject((current) => {
                            const next = new Set(current);
                            next.add(project.id);
                            return next;
                          });
                        }}
                      >
                        Show more
                        <span className="shelf-clip__count">{hiddenSessionCount}</span>
                      </button>
                    </div>
                  )}
                  {!searching &&
                    sessionsExpandedByProject.has(project.id) &&
                    projectSessions.length > SESSION_LIST_PREVIEW && (
                      <div className="shelf-clip shelf-clip--sessions shelf-clip--expanded">
                        <button
                          className="shelf-clip__more"
                          type="button"
                          title="Show fewer sessions"
                          aria-label="Show fewer sessions"
                          onClick={() => {
                            setSessionsExpandedByProject((current) => {
                              const next = new Set(current);
                              next.delete(project.id);
                              return next;
                            });
                          }}
                        >
                          Show less
                        </button>
                      </div>
                    )}
                </div>
              )}
            </div>
          );
        })}
        {shouldCollapseList && (
          <div className="shelf-clip shelf-clip--projects">
            <div className="shelf-clip__fade" aria-hidden />
            <button
              className="shelf-clip__more"
              type="button"
              title={`Show ${hiddenCount} more project${hiddenCount === 1 ? "" : "s"}`}
              aria-label={`Show ${hiddenCount} more projects`}
              onClick={() => setProjectsExpanded(true)}
            >
              Show more
              <span className="shelf-clip__count">{hiddenCount}</span>
            </button>
          </div>
        )}
        {!searching && projectsExpanded && allVisibleProjects.length > PROJECT_LIST_PREVIEW && (
          <div className="shelf-clip shelf-clip--projects shelf-clip--expanded">
            <button
              className="shelf-clip__more"
              type="button"
              title="Show fewer projects"
              aria-label="Show fewer projects"
              onClick={() => setProjectsExpanded(false)}
            >
              Show less
            </button>
          </div>
        )}
      </div>

      <div className="sidebar-footer">
        {themeButton}
        {notifyButton}
        <button className="sidebar-footer__button sidebar-footer__button--collapse" type="button" title={collapsed ? "Pin projects and sessions open" : "Collapse projects and sessions"} aria-label={collapsed ? "Pin projects and sessions open" : "Collapse projects and sessions"} onClick={collapsed ? onExpand : onCollapse}>
          {collapsed ? <PanelLeftOpen size={14} /> : <PanelLeftClose size={14} />}
        </button>
      </div>
      </div>
    </section>
  );
}
