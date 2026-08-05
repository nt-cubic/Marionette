import { useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Bell, BellOff, ChevronDown, ChevronRight, Folder, GripVertical, Moon, PanelLeftClose, PanelLeftOpen, Pencil, Plus, Search, Settings2, Sun, Trash2 } from "lucide-react";
import type { AgentConfig, Project, Session } from "../lib/types";

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

/** Newest activity first. Selecting a dialog must NOT reorder — only send/create bumps recency. */
function sortSessionsNewestFirst(list: Session[]): Session[] {
  return [...list].sort((a, b) => sessionRecency(b) - sessionRecency(a));
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
  /** Manual rename — always persists (like first-message auto-title). */
  onRenameSession?: (sessionId: string, label: string) => void;
  /**
   * Drag reorder. `place` is where the dragged row lands relative to the target:
   * before = insert above the line, after = insert below the line.
   */
  onReorderProjects?: (
    fromProjectId: string,
    toProjectId: string,
    place: "before" | "after"
  ) => void;
};

type DropHint = { targetId: string; place: "before" | "after" };

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
  onReorderProjects,
  searchHitIds = null,
  onSearchQueryChange,
}: ProjectShelfProps) {
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(() => new Set(projects.map((project) => project.id)));
  const [query, setQuery] = useState("");
  const [projectsExpanded, setProjectsExpanded] = useState(false);
  /** Project ids whose session list is fully expanded past the preview cap. */
  const [sessionsExpandedByProject, setSessionsExpandedByProject] = useState<Set<string>>(() => new Set());
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);
  /**
   * Pointer-based reorder (not HTML5 DnD).
   * WebView2 often won't start a drag from nested <button>s, and React state
   * set in dragstart is too late for dragover preventDefault — both broke order.
   */
  const [draggingProjectId, setDraggingProjectId] = useState<string | null>(null);
  /** Insertion line: which row edge the drop will land on. */
  const [dropHint, setDropHint] = useState<DropHint | null>(null);
  const projectDragRef = useRef<{
    fromId: string;
    pointerId: number;
    startX: number;
    startY: number;
    armed: boolean;
  } | null>(null);
  const dropHintRef = useRef<DropHint | null>(null);
  dropHintRef.current = dropHint;

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

  const endProjectDrag = (commit: boolean) => {
    const drag = projectDragRef.current;
    projectDragRef.current = null;
    const fromId = drag?.fromId ?? null;
    const hint = dropHintRef.current;
    setDraggingProjectId(null);
    setDropHint(null);
    dropHintRef.current = null;
    document.body.classList.remove("is-project-reordering");
    if (
      !commit ||
      !drag?.armed ||
      !fromId ||
      !hint ||
      !onReorderProjects
    ) {
      return;
    }
    // No-op if the line sits on an edge that leaves order unchanged.
    onReorderProjects(fromId, hint.targetId, hint.place);
  };

  const onProjectGripPointerDown = (projectId: string, event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!onReorderProjects || searching || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      /* capture optional */
    }
    projectDragRef.current = {
      fromId: projectId,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      armed: false,
    };
    setDraggingProjectId(projectId);
    setDropHint(null);
    dropHintRef.current = null;
    document.body.classList.add("is-project-reordering");
  };

  const onProjectGripPointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = projectDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const dist = Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY);
    if (!drag.armed && dist < 4) return;
    drag.armed = true;
    // elementFromPoint under capture still sees targets below the grip.
    const el = document.elementFromPoint(event.clientX, event.clientY);
    const group = el?.closest("[data-project-id]") as HTMLElement | null;
    const overId = group?.dataset.projectId ?? null;
    if (!overId || overId === drag.fromId) {
      // Still over self (or gap): clear line so we don't lie about the drop.
      if (dropHintRef.current) {
        dropHintRef.current = null;
        setDropHint(null);
      }
      return;
    }
    // Half-row rule: top half → insert before, bottom half → insert after.
    const rowEl = group?.querySelector(".project-row") as HTMLElement | null;
    const box = (rowEl ?? group)!.getBoundingClientRect();
    const place: "before" | "after" =
      event.clientY < box.top + box.height / 2 ? "before" : "after";
    const next: DropHint = { targetId: overId, place };
    const prev = dropHintRef.current;
    if (!prev || prev.targetId !== next.targetId || prev.place !== next.place) {
      dropHintRef.current = next;
      setDropHint(next);
    }
  };

  const onProjectGripPointerUp = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = projectDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      /* ignore */
    }
    endProjectDrag(true);
  };

  const onProjectGripPointerCancel = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = projectDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    endProjectDrag(false);
  };

  const themeButton = (
    <button
      className="pill-action pill-action--icon pill-action--sm sidebar-footer__button"
      type="button"
      title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
      aria-label={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
      onClick={onToggleTheme}
    >
      {theme === "dark" ? <Sun size={13} /> : <Moon size={13} />}
    </button>
  );

  const notifyButton = onToggleDesktopNotify ? (
    <button
      className={
        desktopNotifyEnabled
          ? "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-notify-on"
          : "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-notify-off"
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
      {desktopNotifyEnabled ? <Bell size={13} /> : <BellOff size={13} />}
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
          // Recency only — click-to-select must not jump the row to the top.
          const projectSessions = sortSessionsNewestFirst(rawSessions);
          const sessionsFullyExpanded = searching || sessionsExpandedByProject.has(project.id);
          const shouldCollapseSessions =
            !sessionsFullyExpanded && projectSessions.length > SESSION_LIST_PREVIEW;
          const visibleSessions = shouldCollapseSessions
            ? projectSessions.slice(0, SESSION_LIST_PREVIEW)
            : projectSessions;
          const hiddenSessionCount = Math.max(0, projectSessions.length - visibleSessions.length);

          const showLineBefore =
            dropHint?.targetId === project.id &&
            dropHint.place === "before" &&
            draggingProjectId != null &&
            draggingProjectId !== project.id;
          const showLineAfter =
            dropHint?.targetId === project.id &&
            dropHint.place === "after" &&
            draggingProjectId != null &&
            draggingProjectId !== project.id;

          const rowClass = [
            "project-row",
            project.id === currentProjectId ? "is-active" : "",
            draggingProjectId === project.id ? "is-dragging" : "",
          ]
            .filter(Boolean)
            .join(" ");

          return (
            <div className="project-group" key={project.id} data-project-id={project.id}>
              {showLineBefore && (
                <div className="project-drop-line" aria-hidden>
                  <span className="project-drop-line__dot" />
                  <span className="project-drop-line__bar" />
                </div>
              )}
              <div className={rowClass}>
                {onReorderProjects && !searching ? (
                  <button
                    className="project-row__grip"
                    type="button"
                    title="Drag to reorder projects"
                    aria-label={`Reorder ${project.name}`}
                    onPointerDown={(e) => onProjectGripPointerDown(project.id, e)}
                    onPointerMove={onProjectGripPointerMove}
                    onPointerUp={onProjectGripPointerUp}
                    onPointerCancel={onProjectGripPointerCancel}
                  >
                    <GripVertical size={13} />
                  </button>
                ) : (
                  <span className="project-row__grip project-row__grip--spacer" aria-hidden />
                )}
                <button className="project-row__toggle" type="button" title={expandedProjects.has(project.id) ? "Collapse project" : "Expand project"} aria-label={expandedProjects.has(project.id) ? `Collapse ${project.name}` : `Expand ${project.name}`} aria-expanded={expandedProjects.has(project.id)} onClick={() => toggleProject(project.id)}>
                  {expandedProjects.has(project.id) ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                </button>
                <button
                  className="project-row__select"
                  type="button"
                  title={project.rootPath}
                  onClick={() => onProjectSelect(project.id)}
                >
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
              {showLineAfter && (
                <div className="project-drop-line" aria-hidden>
                  <span className="project-drop-line__dot" />
                  <span className="project-drop-line__bar" />
                </div>
              )}

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
                          session.id === currentSessionId
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
        <button className="pill-action pill-action--icon pill-action--sm sidebar-footer__button sidebar-footer__button--collapse" type="button" title={collapsed ? "Pin projects and sessions open" : "Collapse projects and sessions"} aria-label={collapsed ? "Pin projects and sessions open" : "Collapse projects and sessions"} onClick={collapsed ? onExpand : onCollapse}>
          {collapsed ? <PanelLeftOpen size={13} /> : <PanelLeftClose size={13} />}
        </button>
      </div>
      </div>
    </section>
  );
}
