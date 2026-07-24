import { useEffect, useMemo, useState } from "react";
import { Circle, ChevronDown, ChevronRight, Folder, Moon, PanelLeftClose, PanelLeftOpen, Plus, Search, Settings2, Square, Sun, Trash2 } from "lucide-react";
import type { AgentConfig, Project, Session, SessionStatus } from "../lib/types";

type ThemeMode = "dark" | "light";

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
  onAddProject: () => void;
  onNewSession: (projectId: string) => void;
  onProjectSelect: (projectId: string) => void;
  onSessionSelect: (session: Session) => void;
  onDeleteSession: (sessionId: string) => void;
};

const statusIcon: Record<SessionStatus, typeof Circle> = {
  starting: Circle,
  running: Circle,
  waiting: Circle,
  exited: Square,
  error: Circle
};

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
  onDeleteSession,
  searchHitIds = null,
  onSearchQueryChange,
}: ProjectShelfProps) {
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(() => new Set(projects.map((project) => project.id)));
  const [query, setQuery] = useState("");

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

  if (collapsed) {
    return (
      <div className="collapsed-rail">
        {themeButton}
        <button className="sidebar-footer__button" type="button" title="Show projects and sessions" aria-label="Show projects and sessions" onClick={onExpand}>
          <PanelLeftOpen size={14} />
        </button>
      </div>
    );
  }

  const visibleProjects = filtered.projects;

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

      <div className="project-list" aria-label="Projects with sessions">
        {visibleProjects.length === 0 && (
          <div className="project-list__empty">
            {query.trim() ? `No threads match “${query.trim()}”` : "No projects yet"}
          </div>
        )}
        {visibleProjects.map((project) => {
          const projectSessions =
            filtered.sessionsByProject?.get(project.id) ??
            sessions.filter((session) => session.projectId === project.id);
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
                  {projectSessions.map((session) => {
                    const StatusIcon = statusIcon[session.status] ?? Circle;
                    return (
                      <div
                        className={session.id === currentSessionId ? "session-row is-active" : "session-row"}
                        key={session.id}
                      >
                        <button
                          className="session-row__select"
                          type="button"
                          title={`${agentLabel(session.agentId)} · ${session.status}`}
                          onClick={() => onSessionSelect(session)}
                        >
                          <StatusIcon size={10} className={`session-row__status is-${session.status}`} />
                          <span className="session-row__content">
                            <strong>{session.label}</strong>
                            <em>{agentLabel(session.agentId)}</em>
                          </span>
                        </button>
                        <button className="session-row__delete" type="button" title={`Delete ${session.label}`} aria-label={`Delete ${session.label}`} onClick={() => onDeleteSession(session.id)}>
                          <Trash2 size={12} />
                        </button>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </div>

      <div className="sidebar-footer">
        {themeButton}
        <button className="sidebar-footer__button sidebar-footer__button--collapse" type="button" title="Collapse projects and sessions" aria-label="Collapse projects and sessions" onClick={onCollapse}>
          <PanelLeftClose size={14} />
        </button>
      </div>
      </div>
    </section>
  );
}
