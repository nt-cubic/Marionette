import { useEffect, useState } from "react";
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
  onDeleteSession
}: ProjectShelfProps) {
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(() => new Set(projects.map((project) => project.id)));

  useEffect(() => {
    setExpandedProjects((current) => new Set([...current, ...projects.map((project) => project.id)]));
  }, [projects]);

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

  return (
    <section className="project-tree">
      <div className="sidebar-title">
        <div className="sidebar-search">
          <Search size={13} />
          <span>Search threads...</span>
        </div>
      </div>
      <button className="project-add" type="button" title="Add project" onClick={onAddProject}>
        <Plus size={13} />
        Add project
      </button>

      <div className="project-list" aria-label="Projects with sessions">
        {projects.map((project) => (
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

            {expandedProjects.has(project.id) && <div className="project-sessions">
              {sessions.filter((session) => session.projectId === project.id).map((session) => {
                const agent = agents.find((candidate) => candidate.id === session.agentId);
                  const StatusIcon = statusIcon[session.status];

                  return (
                    <div className={session.id === currentSessionId ? "session-row is-active" : "session-row"} key={session.id}>
                      <button className="session-row__select" type="button" onClick={() => onSessionSelect(session)}>
                        <span className={`session-row__status status-${session.status}`}>
                          <StatusIcon size={8} fill="currentColor" />
                        </span>
                        <span className="session-row__main">
                          <strong>{session.label}</strong>
                          <small>{agent?.label ?? session.agentId}</small>
                        </span>
                      </button>
                      <button className="session-row__delete" type="button" title={`Delete ${session.label}`} aria-label={`Delete ${session.label}`} onClick={() => onDeleteSession(session.id)}>
                        <Trash2 size={13} />
                      </button>
                    </div>
                  );
              })}
            </div>}
          </div>
        ))}
      </div>
      <div className="sidebar-footer">
        {themeButton}
        <button className="sidebar-footer__button sidebar-footer__button--collapse" type="button" title="Collapse projects and sessions" aria-label="Collapse projects and sessions" onClick={onCollapse}>
          <PanelLeftClose size={14} />
        </button>
      </div>
    </section>
  );
}
