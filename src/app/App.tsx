import { listen } from "@tauri-apps/api/event";
import { X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { agents, projects, sessionEvents, sessions } from "../lib/mockData";
import { addProject, cancelAcpSession, createSession as createSessionApi, deleteSession as deleteSessionApi, isTauriRuntime, listAgents, listProjects, listSessions, sendAcpPrompt, stopAcpSession, stopTerminal, writeTerminal } from "../lib/api";
import type { Project, Session, SessionViewMode, TerminalOutput, UsageSnapshot } from "../lib/types";
import { Composer } from "../components/Composer";
import { ContextPanel } from "../components/ContextPanel";
import { ProjectShelf } from "../components/ProjectShelf";
import { SessionView } from "../components/SessionView";

type ThemeMode = "dark" | "light";

export function App() {
  const [availableProjects, setAvailableProjects] = useState<Project[]>(projects);
  const [availableAgents, setAvailableAgents] = useState(agents);
  const [availableSessions, setAvailableSessions] = useState<Session[]>(sessions);
  const [currentProjectId, setCurrentProjectId] = useState(projects[0]?.id ?? "");
  const [currentSessionId, setCurrentSessionId] = useState(sessions[0]?.id ?? "");
  const [openSessionIds, setOpenSessionIds] = useState<string[]>([sessions[0]?.id ?? ""]);
  const [viewMode, setViewMode] = useState<SessionViewMode>("raw-terminal");
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const [projectPath, setProjectPath] = useState("");
  const [projectError, setProjectError] = useState("");
  const [projectAdding, setProjectAdding] = useState(false);
  const [theme, setTheme] = useState<ThemeMode>(() => {
    const stored = window.localStorage.getItem("agentshell-theme");
    return stored === "light" ? "light" : "dark";
  });
  const usage = useMemo<UsageSnapshot>(() => {
    const activeSession = availableSessions.find((session) => session.id === currentSessionId);
    const agent = availableAgents.find((candidate) => candidate.id === activeSession?.agentId) ?? availableAgents[0] ?? agents[0];
    const windows = agent.id === "codex" || agent.id === "claude-code"
      ? [
          { id: "five-hour", label: "5-hour limit", percentage: null },
          { id: "weekly", label: "Weekly limit", percentage: null }
        ]
      : [{ id: "provider", label: "Provider usage", percentage: null }];

    return { agentId: agent.id, agentLabel: agent.label, windows, refreshedAt: "Not connected" };
  }, [availableAgents, availableSessions, currentSessionId]);

  useEffect(() => {
    void Promise.all([listProjects(), listAgents()]).then(async ([nextProjects, nextAgents]) => {
      const resolvedProjects = nextProjects.length > 0 || isTauriRuntime() ? nextProjects : projects;
      setAvailableProjects(resolvedProjects);
      if (resolvedProjects.length > 0) {
        setCurrentProjectId((current) => resolvedProjects.some((project) => project.id === current) ? current : resolvedProjects[0].id);
      } else {
        setCurrentProjectId("");
      }
      setAvailableAgents(nextAgents.length > 0 ? nextAgents : agents);

      const loadedSessions = (await Promise.all(resolvedProjects.map((project) => listSessions(project.id)))).flat();
      if (loadedSessions.length > 0 || isTauriRuntime()) {
        setAvailableSessions(loadedSessions);
        if (loadedSessions[0]) {
          setCurrentProjectId(loadedSessions[0].projectId);
          setCurrentSessionId(loadedSessions[0].id);
          setOpenSessionIds([loadedSessions[0].id]);
          setViewMode(loadedSessions[0].viewMode);
        } else {
          setCurrentSessionId("");
          setOpenSessionIds([]);
        }
      }
    });
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlisten: (() => void) | undefined;
    void listen<TerminalOutput>("session-output", (event) => {
      const output = event.payload;
      if (output.exited) {
        setAvailableSessions((current) => current.map((session) => session.id === output.sessionId ? { ...session, status: "exited" } : session));
      } else if (output.error) {
        setAvailableSessions((current) => current.map((session) => session.id === output.sessionId ? { ...session, status: "error" } : session));
      }
    }).then((dispose) => {
      unlisten = dispose;
    });
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
    window.localStorage.setItem("agentshell-theme", theme);
  }, [theme]);

  const currentProject = useMemo(
    () => availableProjects.find((project) => project.id === currentProjectId) ?? availableProjects[0] ?? (isTauriRuntime() ? undefined : projects[0]),
    [availableProjects, currentProjectId]
  );

  const projectSessions = useMemo(
    () => availableSessions.filter((session) => session.projectId === currentProject?.id),
    [availableSessions, currentProject]
  );

  const currentSession = useMemo(
    () =>
      projectSessions.find((session) => session.id === currentSessionId) ??
      projectSessions[0],
    [currentSessionId, projectSessions]
  );

  const displaySession = useMemo(
    () => currentSession ?? {
      id: `session-empty-${currentProject?.id ?? "none"}`,
      projectId: currentProject?.id ?? "",
      agentId: availableAgents[0]?.id ?? agents[0].id,
      label: "New session",
      cwd: currentProject?.rootPath ?? "",
      status: "exited" as const,
      processId: null,
      ptyId: null,
      startedAt: "",
      lastActiveAt: "",
      rawLogPath: "",
      transcriptPath: "",
      handoffPath: "",
      viewMode: "raw-terminal" as const
    },
    [availableAgents, currentProject, currentSession]
  );

  const currentAgent = useMemo(
    () => availableAgents.find((agent) => agent.id === displaySession.agentId) ?? availableAgents[0] ?? agents[0],
    [availableAgents, displaySession]
  );

  const currentEvents = sessionEvents.filter((event) => event.sessionId === displaySession.id);

  const openSessions = useMemo(
    () => openSessionIds
      .map((sessionId) => availableSessions.find((session) => session.id === sessionId))
      .filter((session): session is Session => Boolean(session)),
    [availableSessions, openSessionIds]
  );

  const createSessionForProject = async (projectId: string, agentId = (availableAgents[0] ?? agents[0]).id) => {
    const project = availableProjects.find((item) => item.id === projectId);
    if (!project) return;

    const newSession = await createSessionApi(projectId, agentId);
    if (!newSession) return;
    setAvailableSessions((current) => [...current, newSession]);
    setOpenSessionIds((current) => [...current.filter((id) => id !== newSession.id), newSession.id]);
    setCurrentProjectId(projectId);
    setCurrentSessionId(newSession.id);
    setViewMode("raw-terminal");
  };

  const openSession = (nextSession: Session) => {
    setOpenSessionIds((current) => current.includes(nextSession.id) ? current : [...current, nextSession.id]);
    setCurrentProjectId(nextSession.projectId);
    setCurrentSessionId(nextSession.id);
    setViewMode(nextSession.viewMode);
  };

  const closeSessionTab = (sessionId: string) => {
    const nextOpenIds = openSessionIds.filter((id) => id !== sessionId);
    if (nextOpenIds.length === 0) {
      void createSessionForProject(currentProject?.id ?? availableProjects[0]?.id ?? "");
      return;
    }

    setOpenSessionIds(nextOpenIds);
    if (sessionId === currentSessionId) {
      const nextSession = availableSessions.find((session) => session.id === nextOpenIds[nextOpenIds.length - 1]);
      if (nextSession) openSession(nextSession);
    }
  };

  const deleteSession = (sessionId: string) => {
    const deletedSession = availableSessions.find((session) => session.id === sessionId);
    if (deletedSession && (deletedSession.status === "starting" || deletedSession.status === "running" || deletedSession.status === "waiting")) {
      const deletedAgent = availableAgents.find((agent) => agent.id === deletedSession.agentId);
      void (deletedAgent?.transport === "acp" ? stopAcpSession(sessionId) : stopTerminal(sessionId)).catch(() => undefined);
    }
    if (deletedSession) void deleteSessionApi(deletedSession.projectId, sessionId).catch(() => undefined);
    setAvailableSessions((current) => current.filter((session) => session.id !== sessionId));
    setOpenSessionIds((current) => current.filter((id) => id !== sessionId));
    if (sessionId === currentSessionId) {
      const nextSession = availableSessions.find((session) => session.id !== sessionId && session.projectId === currentProject?.id);
      if (nextSession) openSession(nextSession);
      else void createSessionForProject(currentProject?.id ?? availableProjects[0]?.id ?? "");
    }
  };

  const handleAgentChange = async (agentId: string) => {
    if (!currentProject) return;

    // If no current session, create one
    if (!currentSession || !currentSessionId) {
      void createSessionForProject(currentProject.id, agentId);
      return;
    }

    // Stop the old agent's process
    const oldAgent = availableAgents.find((a) => a.id === currentSession.agentId);
    if (oldAgent?.transport === "acp") {
      try { await stopAcpSession(currentSessionId); } catch { /* ok */ }
    } else if (oldAgent) {
      try { await stopTerminal(currentSessionId); } catch { /* ok */ }
    }

    // Update session metadata — SessionView will restart with the new agent
    setAvailableSessions((current) =>
      current.map((s) =>
        s.id === currentSessionId
          ? { ...s, agentId, status: "starting" as const }
          : s
      )
    );
  };

  const handleSessionStatusChange = useCallback((status: Session["status"]) => {
    setAvailableSessions((current) =>
      current.map((session) => (session.id === currentSessionId ? { ...session, status } : session))
    );
  }, [currentSessionId]);

  const handleInterrupt = async () => {
    if (!currentSessionId) return;
    if (currentAgent.transport === "acp") {
      await cancelAcpSession(currentSessionId);
      handleSessionStatusChange("waiting");
    } else {
      await stopTerminal(currentSessionId);
      handleSessionStatusChange("exited");
    }
  };

  const handleSend = async (text: string) => {
    if (!currentSessionId) return;
    handleSessionStatusChange("running");
    try {
      if (currentAgent.transport === "acp") {
        await sendAcpPrompt(currentSessionId, text);
        handleSessionStatusChange("waiting");
      } else {
        await writeTerminal(currentSessionId, `${text}\r`);
      }
    } catch {
      handleSessionStatusChange("error");
    }
  };

  const handleAddProject = async () => {
    const path = projectPath.trim();
    if (!path) {
      setProjectError("Enter a project folder path.");
      return;
    }
    setProjectAdding(true);
    setProjectError("");
    try {
      const project = await addProject(path);
      setAvailableProjects((current) => [...current.filter((item) => item.id !== project.id), project]);
      setCurrentProjectId(project.id);
      setProjectDialogOpen(false);
      setProjectPath("");
    } catch (error) {
      setProjectError(String(error));
    } finally {
      setProjectAdding(false);
    }
  };

  return (
    <main className="app-shell">
      <div className={`workspace-grid${leftCollapsed ? " is-left-collapsed" : ""}${rightCollapsed ? " is-right-collapsed" : ""}`}>
        <aside className={leftCollapsed ? "left-rail is-collapsed" : "left-rail"} aria-label="Projects and sessions">
          <ProjectShelf
            agents={availableAgents}
            projects={availableProjects}
            sessions={availableSessions}
            currentProjectId={currentProjectId}
            currentSessionId={displaySession.id}
            collapsed={leftCollapsed}
            theme={theme}
            onCollapse={() => setLeftCollapsed(true)}
            onExpand={() => setLeftCollapsed(false)}
            onToggleTheme={() => setTheme((current) => (current === "dark" ? "light" : "dark"))}
            onAddProject={() => { setProjectDialogOpen(true); setProjectError(""); }}
            onNewSession={(projectId) => {
              createSessionForProject(projectId);
            }}
            onProjectSelect={(projectId) => {
              setCurrentProjectId(projectId);
              const nextSession = availableSessions.find((session) => session.projectId === projectId);
              if (nextSession) {
                openSession(nextSession);
              } else {
                setCurrentSessionId("");
                setViewMode("raw-terminal");
              }
            }}
            onSessionSelect={openSession}
            onDeleteSession={deleteSession}
          />
        </aside>

        <section className="center-workspace" aria-label="Active workspace">
          <SessionView
            agent={currentAgent}
            events={currentEvents}
            session={displaySession}
            viewMode={viewMode}
            openSessions={openSessions}
            onTabSelect={openSession}
            onTabClose={closeSessionTab}
            onNewTab={() => createSessionForProject(currentProject?.id ?? "")}
            onSessionStatusChange={handleSessionStatusChange}
            onViewModeToggle={() => setViewMode(viewMode === "raw-terminal" ? "clean" : "raw-terminal")}
          />
          <Composer
            agent={currentAgent}
            agents={availableAgents}
            currentAgentId={displaySession.agentId}
            sessionId={displaySession.id}
            sessionStatus={displaySession.status}
            onAgentChange={handleAgentChange}
            onInterrupt={() => void handleInterrupt()}
            onSend={(text) => void handleSend(text)}
          />
        </section>

        <ContextPanel
          collapsed={rightCollapsed}
          onCollapse={() => setRightCollapsed(true)}
          onExpand={() => setRightCollapsed(false)}
          usage={usage}
          onUsageRefresh={() => undefined}
        />
      </div>
      {projectDialogOpen && (
        <div className="project-dialog-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !projectAdding) setProjectDialogOpen(false); }}>
          <form className="project-dialog" role="dialog" aria-modal="true" aria-labelledby="project-dialog-title" onSubmit={(event) => { event.preventDefault(); void handleAddProject(); }}>
            <div className="project-dialog__header">
              <div>
                <strong id="project-dialog-title">Add project</strong>
                <span>Open a local folder as a project</span>
              </div>
              <button className="project-dialog__close" type="button" title="Close" aria-label="Close" onClick={() => setProjectDialogOpen(false)} disabled={projectAdding}><X size={14} /></button>
            </div>
            <label className="project-dialog__field">
              <span>Folder path</span>
              <input autoFocus value={projectPath} onChange={(event) => setProjectPath(event.target.value)} placeholder="D:\\Work\\MyProject" spellCheck={false} />
            </label>
            {projectError && <p className="project-dialog__error">{projectError}</p>}
            <div className="project-dialog__actions">
              <button type="button" className="project-dialog__cancel" onClick={() => setProjectDialogOpen(false)} disabled={projectAdding}>Cancel</button>
              <button type="submit" className="project-dialog__submit" disabled={projectAdding}>{projectAdding ? "Adding..." : "Add project"}</button>
            </div>
          </form>
        </div>
      )}
    </main>
  );
}
