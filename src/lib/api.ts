import { invoke } from "@tauri-apps/api/core";
import { agents as mockAgents, projects as mockProjects, sessions as mockSessions } from "./mockData";
import type { AgentConfig, AgentCommandStatus, CapabilitySnapshot, Project, Session } from "./types";

export function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function listProjects(): Promise<Project[]> {
  if (!isTauriRuntime()) return mockProjects;
  try {
    return await invoke<Project[]>("list_projects");
  } catch {
    return [];
  }
}

export async function addProject(path: string): Promise<Project> {
  if (isTauriRuntime()) return invoke<Project>("add_project", { path });

  const now = new Date().toISOString();
  return {
    id: `project-${Date.now()}`,
    name: path.split(/[\\/]/).filter(Boolean).pop() ?? "Project",
    rootPath: path,
    createdAt: now,
    lastOpenedAt: now
  };
}

export async function listAgents(): Promise<AgentConfig[]> {
  if (!isTauriRuntime()) return mockAgents;
  try {
    return await invoke<AgentConfig[]>("list_agents");
  } catch {
    return mockAgents;
  }
}

export async function testAgentCommand(agentId: string): Promise<AgentCommandStatus> {
  if (!isTauriRuntime()) {
    return { id: agentId, status: "failed", path: null, message: "Command check is available in the desktop app" };
  }
  return invoke<AgentCommandStatus>("test_agent_command", { agentId });
}

export async function listSessions(projectId: string): Promise<Session[]> {
  if (!isTauriRuntime()) return mockSessions.filter((session) => session.projectId === projectId);
  try {
    return await invoke<Session[]>("list_sessions", { projectId });
  } catch {
    return [];
  }
}

export async function createSession(projectId: string, agentId: string, label = "New session"): Promise<Session | null> {
  if (isTauriRuntime()) return invoke<Session>("create_session", { projectId, agentId, label });
  const project = mockProjects.find((candidate) => candidate.id === projectId);
  if (!project) return null;
  const now = new Date().toISOString();
  const id = `session-${Date.now()}`;
  return {
    id,
    projectId,
    agentId,
    label,
    cwd: project.rootPath,
    status: "exited",
    processId: null,
    ptyId: null,
    startedAt: "",
    lastActiveAt: now,
    rawLogPath: `${project.rootPath}\\.agentshell\\sessions\\${id}.raw.log`,
    transcriptPath: `${project.rootPath}\\.agentshell\\transcripts\\${id}.jsonl`,
    handoffPath: `${project.rootPath}\\.agentshell\\handoff.md`,
    viewMode: "raw-terminal"
  };
}

export async function deleteSession(projectId: string, sessionId: string): Promise<void> {
  if (isTauriRuntime()) await invoke("delete_session", { projectId, sessionId });
}

export async function startTerminal(sessionId: string, cwd: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("start_terminal", { sessionId, cwd });
}

export async function readTerminalSnapshot(sessionId: string, cwd: string): Promise<string> {
  if (!isTauriRuntime()) return "";
  return invoke<string>("read_terminal_snapshot", { sessionId, cwd });
}

export async function startAcpSession(
  sessionId: string,
  command: string,
  args: string[],
  cwd: string
): Promise<CapabilitySnapshot | null> {
  if (!isTauriRuntime()) return null;
  return invoke<CapabilitySnapshot>("start_acp_session", { sessionId, command, args, cwd });
}

export async function sendAcpPrompt(sessionId: string, text: string): Promise<unknown> {
  if (!isTauriRuntime()) return null;
  return invoke("send_acp_prompt", { sessionId, text });
}

export async function cancelAcpSession(sessionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("cancel_acp_session", { sessionId });
}

export async function stopAcpSession(sessionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("stop_acp_session", { sessionId });
}

export async function writeTerminal(sessionId: string, data: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("write_terminal", { sessionId, data });
}

export async function resizeTerminal(sessionId: string, cols: number, rows: number): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("resize_terminal", { sessionId, cols, rows });
}

export async function stopTerminal(sessionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("stop_terminal", { sessionId });
}

// ─── ACP Capability API ─────────────────────────────────────────────────────

export async function getSessionCapabilities(sessionId: string): Promise<CapabilitySnapshot | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<CapabilitySnapshot | null>("get_session_capabilities", { sessionId });
  } catch {
    return null;
  }
}

export async function updateAcpSession(sessionId: string, config: Record<string, unknown>): Promise<unknown> {
  if (!isTauriRuntime()) return null;
  return invoke("update_acp_session", { sessionId, config });
}
