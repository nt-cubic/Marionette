import { invoke } from "@tauri-apps/api/core";
import { agents as mockAgents, projects as mockProjects, sessions as mockSessions, memoryStore as mockMemoryStore, dshPlugins as mockDshPlugins } from "./mockData";
import type { AgentConfig, AgentCommandStatus, CapabilitySnapshot, Project, ProviderInfo, ProxyConfig, ProxyTestResult, Session, SessionLabelSource } from "./types";

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

/**
 * Folder from Explorer "在此处打开 Marionette" / `--open-path` (consumed once).
 * Null when the app was started normally.
 */
export async function takeLaunchOpenPath(): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  try {
    const path = await invoke<string | null>("take_launch_open_path");
    const trimmed = path?.trim();
    return trimmed ? trimmed : null;
  } catch {
    return null;
  }
}

/** Event name when a second process hands off an open-here path. */
export const OPEN_PATH_EVENT = "marionette-open-path";

/** Virtual project ID for chat sessions. */
export const CHAT_PROJECT_ID = "project-chat";

export type OpenHereResult =
  | { kind: "project"; project: Project }
  | { kind: "chat"; session: Session };

/** Get the default folder for chat — `--open-path` if set, otherwise current working directory. */
export async function getDefaultFolder(): Promise<string> {
  if (!isTauriRuntime()) return "";
  try {
    return (await invoke<string>("get_default_folder")) ?? "";
  } catch {
    return "";
  }
}

/** Open a folder — as a project if it has `.marionette`, otherwise as a chat session. */
export async function openHere(path: string, agentId: string): Promise<OpenHereResult> {
  if (isTauriRuntime()) {
    return invoke<OpenHereResult>("open_here", { path, agentId });
  }
  const now = new Date().toISOString();
  const id = `session-chat-${Date.now()}`;
  return {
    kind: "chat",
    session: {
      id,
      projectId: CHAT_PROJECT_ID,
      agentId,
      label: `Chat · ${path.split(/[\\/]/).filter(Boolean).pop() ?? "Folder"}`,
      cwd: path,
      status: "exited",
      processId: null,
      startedAt: "",
      lastActiveAt: now,
      transcriptPath: "",
      handoffPath: "",
      viewMode: "clean",
    },
  };
}

/** List chat sessions (sessions under the virtual chat project). */
export async function listChatSessions(): Promise<Session[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<Session[]>("list_sessions", { projectId: CHAT_PROJECT_ID });
  } catch {
    return [];
  }
}

/** Native folder picker. Returns null if the user cancels. */
export async function pickFolder(): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<string | null>("pick_folder");
  } catch {
    return null;
  }
}

/** Native multi-file picker with absolute paths. Empty if cancelled. */
export async function pickFiles(): Promise<string[]> {
  if (!isTauriRuntime()) return [];
  try {
    return (await invoke<string[]>("pick_files")) ?? [];
  } catch {
    return [];
  }
}

/** Remove project from Marionette list (does not delete workspace files). */
export async function deleteProject(projectId: string): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("delete_project", { projectId });
    return;
  }
  // Browser mock: caller updates local state.
}

/** Persist project shelf order (`projects.json` array order). */
export async function reorderProjects(orderedIds: string[]): Promise<Project[]> {
  if (!isTauriRuntime()) return mockProjects;
  return invoke<Project[]>("reorder_projects", { orderedIds });
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

/** Status of every agent CLI in one round-trip (bridge + the CLIs it drives). */
export async function listAgentCommands(): Promise<AgentCommandStatus[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<AgentCommandStatus[]>("list_agent_commands");
  } catch {
    return [];
  }
}

/**
 * Installed (and optionally published) versions for every agent.
 *
 * `checkRegistry` costs a network round-trip, so the menu opens without it and
 * asks again in the background.
 */
export async function agentVersions(
  checkRegistry: boolean
): Promise<import("./types").AgentVersionInfo[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<import("./types").AgentVersionInfo[]>("agent_versions", { checkRegistry });
  } catch {
    return [];
  }
}

/**
 * Install the agent's ACP command with npm (package comes from the Rust table).
 *
 * Pass `force: true` to reinstall even when the CLI is already on PATH — that is
 * the update path. Without force, an installed agent is a no-op.
 */
export async function installAgent(
  agentId: string,
  includeDependencies = true,
  force = false
): Promise<import("./types").AgentInstallResult> {
  if (!isTauriRuntime()) {
    throw new Error("Installing agents is available in the desktop app");
  }
  return invoke<import("./types").AgentInstallResult>("install_agent", {
    agentId,
    includeDependencies,
    force,
  });
}

export type PreflightCheck = {
  checkId: string;
  label: string;
  status: "pass" | "fail" | "warn";
  message: string;
};

export type PreflightResult = {
  agentId: string;
  agentName: string;
  passed: boolean;
  checks: PreflightCheck[];
};

export async function agentPreflight(agentId: string): Promise<PreflightResult | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<PreflightResult>("agent_preflight", { agentId });
  } catch {
    return null;
  }
}

export type CustomAgentDef = {
  id: string;
  label: string;
  command: string;
  args: string[];
  npmPackage?: string | null;
  note?: string | null;
};

export async function addCustomAgent(def: CustomAgentDef): Promise<CustomAgentDef> {
  if (!isTauriRuntime()) throw new Error("Custom agents require the desktop app");
  return invoke<CustomAgentDef>("add_custom_agent", { def });
}

export async function removeCustomAgent(id: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("remove_custom_agent", { id });
}

export async function listCustomAgents(): Promise<CustomAgentDef[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<CustomAgentDef[]>("list_custom_agents");
  } catch {
    return [];
  }
}

export type AcpCatalogChannel = {
  kind: "npx" | "uvx" | "binary" | string;
  package?: string | null;
  cmd?: string | null;
  args: string[];
};

export type AcpCatalogAgent = {
  id: string;
  name: string;
  description: string;
  version: string;
  channels: AcpCatalogChannel[];
  installable: boolean;
};

export async function fetchAcpRegistry(): Promise<AcpCatalogAgent[]> {
  if (!isTauriRuntime()) return [];
  return invoke<AcpCatalogAgent[]>("fetch_acp_registry");
}

export type ExternalRefreshResult = {
  added: number;
  updated: number;
  skipped: number;
  failed: number;
  removed: number;
  redirected: { from: string; to: string }[];
  sessions: Session[];
};

export async function refreshExternalSessions(projectId: string): Promise<ExternalRefreshResult> {
  if (!isTauriRuntime()) {
    return {
      added: 0,
      updated: 0,
      skipped: 0,
      failed: 0,
      removed: 0,
      redirected: [],
      sessions: [],
    };
  }
  return invoke<ExternalRefreshResult>("refresh_external_sessions", { projectId });
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
    startedAt: "",
    lastActiveAt: now,
    transcriptPath: `${project.rootPath}\\.marionette\\transcripts\\${id}.jsonl`,
    handoffPath: `${project.rootPath}\\.marionette\\handoff.md`,
    viewMode: "clean"
  };
}

/** Create a Chat session in the global Chat store. */
export async function createChatSession(
  agentId: string,
  label = "新对话",
  cwd?: string | null,
): Promise<Session | null> {
  if (!isTauriRuntime()) {
    const now = new Date().toISOString();
    const id = `session-chat-${Date.now()}`;
    return {
      id,
      projectId: CHAT_PROJECT_ID,
      agentId,
      label,
      cwd: cwd ?? "",
      status: "exited",
      processId: null,
      startedAt: "",
      lastActiveAt: now,
      transcriptPath: ``,
      handoffPath: ``,
      viewMode: "clean",
    };
  }
  return invoke<Session>("create_chat_session", {
    agentId,
    label,
    cwd: cwd ?? null,
  });
}

/** Hidden child session for `@agent` delegate. */
export async function createChildSession(
  projectId: string,
  parentSessionId: string,
  agentId: string,
  label = "Delegate"
): Promise<Session | null> {
  if (isTauriRuntime()) {
    return invoke<Session>("create_child_session", {
      projectId,
      parentSessionId,
      agentId,
      label,
    });
  }
  const project = mockProjects.find((candidate) => candidate.id === projectId);
  if (!project) return null;
  const now = new Date().toISOString();
  const id = `session-child-${Date.now()}`;
  return {
    id,
    projectId,
    agentId,
    label,
    cwd: project.rootPath,
    status: "exited",
    processId: null,
    startedAt: "",
    lastActiveAt: now,
    transcriptPath: `${project.rootPath}\\.marionette\\transcripts\\${id}.jsonl`,
    handoffPath: `${project.rootPath}\\.marionette\\handoff\\${id}.md`,
    viewMode: "clean",
    parentSessionId,
    origin: "delegate",
  };
}

export async function listChildSessions(parentSessionId: string): Promise<Session[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<Session[]>("list_child_sessions", { parentSessionId });
  } catch {
    return [];
  }
}

export async function deleteSession(projectId: string, sessionId: string): Promise<void> {
  if (isTauriRuntime()) await invoke("delete_session", { projectId, sessionId });
}

/** Persist session ↔ agent binding (source of truth for Composer). */
export async function updateSessionAgent(sessionId: string, agentId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("update_session_agent", { sessionId, agentId });
}

/** Persist per-dialog model / mode / effort (SSOT on disk with session list). */
export async function updateSessionPrefs(
  sessionId: string,
  prefs: import("./types").SessionComposerPrefs
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("update_session_prefs", {
    sessionId,
    preferredModel: prefs.preferredModel ?? null,
    preferredMode: prefs.preferredMode ?? null,
    preferredEffort: prefs.preferredEffort ?? null,
    preferredEffortId: prefs.preferredEffortId ?? null,
    preferredAlwaysApprove: prefs.preferredAlwaysApprove ?? null,
  });
}

export async function updateSessionLabel(
  sessionId: string,
  label: string,
  labelSource: SessionLabelSource = "manual",
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("update_session_label", { sessionId, label, labelSource });
}

/**
 * Pin / unpin a dialog in the left shelf. Returns the saved row (the shelf sorts
 * on `pinnedAt`, so it adopts the authoritative timestamp).
 */
export async function setSessionPinned(
  sessionId: string,
  pinned: boolean,
): Promise<import("./types").Session | null> {
  if (!isTauriRuntime()) return null;
  return invoke<import("./types").Session>("set_session_pinned", { sessionId, pinned });
}

/** Persist dialog runtime status so detached windows load the correct Interrupt/Send state. */
export async function updateSessionStatus(
  sessionId: string,
  status: import("./types").SessionStatus,
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("update_session_status", { sessionId, status });
}

export async function writeTranscript(sessionId: string, events: unknown[]): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("write_transcript", { sessionId, events });
}

export async function loadTranscript(sessionId: string): Promise<unknown[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<unknown[]>("load_transcript", { sessionId });
  } catch {
    return [];
  }
}

export async function searchSessions(query: string): Promise<string[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<string[]>("search_sessions", { query });
  } catch {
    return [];
  }
}

export type AgentAuthProbe = {
  agentId: string;
  status: "logged_in" | "logged_out" | "unknown";
  loggedIn?: boolean;
  message?: string;
  /** Credential-file mtime (ms since epoch). Detects `grok login` rewriting an already-present auth.json. */
  stamp?: number;
  raw?: unknown;
};

export async function probeAgentAuth(
  agentId: string,
  modelId?: string | null
): Promise<AgentAuthProbe | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<AgentAuthProbe>("probe_agent_auth", {
      agentId,
      modelId: modelId ?? null,
    });
  } catch {
    return null;
  }
}

/** Open the agent’s native login flow (Claude: `claude auth login` → browser). */
export async function startAgentLogin(agentId: string): Promise<{ started: boolean; message?: string } | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke("start_agent_login", { agentId });
  } catch (error) {
    return {
      started: false,
      message: error instanceof Error ? error.message : String(error),
    };
  }
}

/** Open `grok update` in its own console. The running session keeps the old binary. */
export async function startGrokUpdate(): Promise<{ started: boolean; message?: string } | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke("start_grok_update");
  } catch (error) {
    return {
      started: false,
      message: error instanceof Error ? error.message : String(error),
    };
  }
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

export async function sendAcpPrompt(
  sessionId: string,
  text: string,
  imagePaths: string[] = [],
): Promise<unknown> {
  if (!isTauriRuntime()) return null;
  return invoke("send_acp_prompt", {
    sessionId,
    text,
    imagePaths: imagePaths.length > 0 ? imagePaths : null,
  });
}

/** Load a local image as a data URL for preview / annotator. */
export async function readImageDataUrl(
  path: string,
): Promise<{ path: string; mimeType: string; dataUrl: string; byteLength: number }> {
  return invoke("read_image_data_url", { path });
}

/**
 * Persist clipboard/paste image bytes under `~/.marionette/clipboard/`.
 * `base64Data` may be raw base64 or a full `data:image/...;base64,...` URL.
 */
export async function savePastedImage(
  base64Data: string,
  mimeType?: string | null,
): Promise<{ path: string; mimeType: string; byteLength: number; name: string }> {
  if (!isTauriRuntime()) {
    throw new Error("Paste image requires the desktop app");
  }
  return invoke("save_pasted_image", {
    base64Data,
    mimeType: mimeType ?? null,
  });
}

export async function cancelAcpSession(sessionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("cancel_acp_session", { sessionId });
}

export async function stopAcpSession(sessionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("stop_acp_session", { sessionId });
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

/** Probe provider balance/usage for an OpenCode-style `provider/model` id. */
export async function probeProviderUsage(modelId?: string | null): Promise<import("./usage").ProviderUsageProbe | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke("probe_provider_usage", { modelId: modelId ?? null });
  } catch {
    return null;
  }
}

/**
 * Grok account weekly credit usage (`_x.ai/billing` on a live ACP session).
 * Same numbers the native TUI `/usage` panel shows.
 */
export async function probeAcpBilling(sessionId: string): Promise<unknown | null> {
  if (!isTauriRuntime() || !sessionId) return null;
  try {
    return await invoke("probe_acp_billing", { sessionId });
  } catch {
    return null;
  }
}

/** Write a line to the local developer diary (`%USERPROFILE%\\.marionette\\logs\\dev.log`). Not a UI surface. */
export async function appendDebugLog(entry: {
  source: string;
  level?: "info" | "warn" | "error";
  sessionId?: string;
  summary: string;
  detail?: string;
}): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await invoke("append_debug_log", {
      source: entry.source,
      level: entry.level ?? "info",
      sessionId: entry.sessionId ?? "",
      summary: entry.summary,
      detail: entry.detail ?? null,
    });
  } catch {
    // Never break the product UI for logging
  }
}

export async function getDebugLogPath(): Promise<string> {
  if (!isTauriRuntime()) return "";
  try {
    return await invoke<string>("debug_log_path");
  } catch {
    return "";
  }
}

export async function generateHandoff(params: {
  projectId: string;
  sessionId: string;
  targetAgentId: string;
  sourceAgentId?: string;
}): Promise<import("./types").HandoffResult | null> {
  if (!isTauriRuntime()) {
    return {
      projectId: params.projectId,
      targetAgentId: params.targetAgentId,
      handoffPath: ".marionette/handoff.md",
      prompt: `Continue from handoff (browser mock) → ${params.targetAgentId}`,
      createdAt: new Date().toISOString(),
    };
  }
  try {
    return await invoke("generate_handoff", {
      projectId: params.projectId,
      sessionId: params.sessionId,
      targetAgentId: params.targetAgentId,
      sourceAgentId: params.sourceAgentId ?? null,
    });
  } catch {
    return null;
  }
}

export async function getChangedFiles(projectId: string): Promise<import("./types").ChangedFile[]> {
  if (!isTauriRuntime()) {
    const { changedFiles } = await import("./mockData");
    return changedFiles;
  }
  try {
    return await invoke("get_changed_files", { projectId });
  } catch {
    return [];
  }
}

/** Current git branch for the project, or null if not a repo / unavailable. */
export async function getCurrentBranch(projectId: string): Promise<string | null> {
  if (!isTauriRuntime()) return "main";
  try {
    const branch = await invoke<string | null>("get_current_branch", { projectId });
    return branch?.trim() || null;
  } catch {
    return null;
  }
}

export async function getFileDiff(projectId: string, path: string): Promise<string> {
  if (!isTauriRuntime()) return "";
  try {
    return await invoke<string>("get_file_diff", { projectId, path });
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

/**
 * Per-file diff fingerprints from one whole-tree `git diff` (two IPC calls per
 * snapshot). Replaces the per-file `getFileDiff` loop that spawned a git
 * process per changed file and stalled sends in Unity-scale repos for minutes.
 */
export async function getFileFingerprints(
  projectId: string,
): Promise<Record<string, string>> {
  if (!isTauriRuntime()) return {};
  try {
    return await invoke<Record<string, string>>("get_file_fingerprints", { projectId });
  } catch {
    return {};
  }
}

export async function respondAcpPermission(requestId: string, optionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("respond_acp_permission", { requestId, optionId });
}

export async function respondAcpQuestion(
  requestId: string,
  answers: { question: string; selected: string[] }[],
  declined = false,
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("respond_acp_question", { requestId, answers, declined });
}

/** Grok exit_plan_mode decision: approve | request_changes | abandon */
export type PlanApprovalDecision = "approve" | "request_changes" | "abandon";

export async function respondAcpPlanApproval(
  requestId: string,
  decision: PlanApprovalDecision,
  feedback?: string | null,
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("respond_acp_plan_approval", {
    requestId,
    decision,
    feedback: feedback ?? null,
  });
}

export type AppUpdateInfo = {
  currentVersion: string;
  latestVersion: string | null;
  updateAvailable: boolean;
  releaseUrl: string | null;
  assetName: string | null;
  assetUrl: string | null;
  notes: string | null;
  note: string | null;
};

export async function checkAppUpdate(): Promise<AppUpdateInfo | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<AppUpdateInfo>("check_app_update");
  } catch {
    return null;
  }
}

export async function downloadAppUpdate(): Promise<{ path: string; version: string } | null> {
  if (!isTauriRuntime()) return null;
  return invoke<{ path: string; version: string }>("download_app_update");
}

export async function applyAppUpdateAndRelaunch(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("apply_app_update_and_relaunch");
}

// ─── Project context: MCP servers + skills lent to agents that lack them ────

/** Scan machine + project for MCP servers and skills (cheap; call on demand). */
export async function scanProjectContext(
  projectId: string
): Promise<import("./types").ProjectContext | null> {
  if (!isTauriRuntime()) {
    const { projectContext } = await import("./mockData");
    return projectId ? { ...projectContext, projectId } : null;
  }
  if (!projectId) return null;
  try {
    return await invoke<import("./types").ProjectContext>("scan_project_context", { projectId });
  } catch {
    return null;
  }
}

export async function setProjectContextEnabled(
  projectId: string,
  kind: "mcp" | "skill",
  id: string,
  enabled: boolean
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("set_project_context_enabled", { projectId, kind, id, enabled });
}

/** Project-level todos (`.marionette/todos.json`). Frontend owns truth. */
export async function listTodos(
  projectId: string
): Promise<import("./todos").TodoItem[]> {
  if (!isTauriRuntime() || !projectId) return [];
  try {
    return await invoke<import("./todos").TodoItem[]>("list_todos", { projectId });
  } catch {
    return [];
  }
}

export async function saveTodos(
  projectId: string,
  items: import("./todos").TodoItem[]
): Promise<void> {
  if (!isTauriRuntime() || !projectId) return;
  await invoke("save_todos", { projectId, items });
}

export type OutsidePath = {
  path: string;
  /** Folder to grant (a file grants its parent). */
  dir: string;
  isDirectory: boolean;
};

/** Which paths in a draft point outside the project and are not granted yet. */
export async function checkOutsideProjectPaths(
  projectId: string,
  paths: string[]
): Promise<OutsidePath[]> {
  if (!isTauriRuntime()) {
    // Browser mock: treat any absolute path as outside so the grant dialog is
    // reachable without the desktop runtime. Desktop asks Rust for the truth.
    return paths
      .filter((path) => /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("/"))
      .map((path) => ({
        path,
        dir: path.slice(0, Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"))),
        isDirectory: false,
      }));
  }
  if (!projectId || paths.length === 0) return [];
  try {
    return await invoke<OutsidePath[]>("check_outside_project_paths", { projectId, paths });
  } catch {
    return [];
  }
}

/**
 * Grant a folder outside the project. Applies via ACP `additionalDirectories`
 * at `session/new`, so a live session has to reconnect to pick it up.
 */
export async function grantWorkspaceRoot(
  projectId: string,
  dir: string,
  sessionId?: string | null
): Promise<{ workspaceRoots: string[]; restartNeeded: boolean }> {
  if (!isTauriRuntime()) return { workspaceRoots: [], restartNeeded: false };
  return invoke("grant_workspace_root", { projectId, dir, sessionId: sessionId ?? null });
}

/** Skills preamble for an agent that has no skill system of its own. */
export async function projectContextPrompt(
  projectId: string,
  agentId: string
): Promise<string | null> {
  if (!isTauriRuntime() || !projectId || !agentId) return null;
  try {
    return await invoke<string | null>("project_context_prompt", { projectId, agentId });
  } catch {
    return null;
  }
}

// ─── Opening paths / URLs found in agent output ─────────────────────────────

export type LinkResolution = {
  kind: "url" | "file" | "directory" | "missing";
  target: string;
  /** Windows would execute this rather than open it — never launched silently. */
  risky: boolean;
};

export async function resolveLinkTarget(
  target: string,
  cwd?: string | null
): Promise<LinkResolution> {
  if (!isTauriRuntime()) {
    const isUrl = /^https?:\/\//i.test(target);
    return { kind: isUrl ? "url" : "missing", target, risky: false };
  }
  return invoke<LinkResolution>("resolve_link_target", { target, cwd: cwd ?? null });
}

/** Hand a file / folder / URL to the OS default handler (user-initiated only). */
export async function openExternal(
  target: string,
  cwd?: string | null,
  force = false
): Promise<{ opened: boolean; reason?: string; message?: string; target?: string }> {
  if (!isTauriRuntime()) {
    return { opened: false, reason: "browser", message: "Opening is available in the desktop app" };
  }
  return invoke("open_external", { target, cwd: cwd ?? null, force });
}

export async function revealInFileManager(target: string, cwd?: string | null): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("reveal_in_file_manager", { target, cwd: cwd ?? null });
}

// ─── Provider API Key Management ────────────────────────────────────────────

/**
 * `force` confirms overwriting an OAuth login. Without it the backend refuses,
 * because replacing `{ type: "oauth", refresh, … }` with an API key throws away
 * a refresh token that only `opencode auth login` can mint again.
 */
export async function saveProviderKey(
  provider: string,
  key: string,
  force = false,
): Promise<void> {
  await invoke("save_provider_key", { provider, key, force });
}

export async function listProviders(): Promise<ProviderInfo[]> {
  return invoke("list_providers");
}

/** `force` confirms deleting an OAuth login — see {@link saveProviderKey}. */
export async function deleteProviderKey(provider: string, force = false): Promise<void> {
  await invoke("delete_provider_key", { provider, force });
}

/** User catalog meta only (`~/.marionette/providers.json`) — does not touch auth.json. */
export async function upsertProviderMeta(
  id: string,
  label: string,
  keyAliases: string[] = [],
  probeStrategy = "none",
): Promise<void> {
  await invoke("upsert_provider_meta", {
    id,
    label,
    keyAliases,
    probeStrategy,
  });
}

export async function deleteProviderMeta(id: string): Promise<void> {
  await invoke("delete_provider_meta", { id });
}

// ─── Cross-Agent Model Bridge (凭据/跨用) ────────────────────────────────────

/** One scanned agent in a `bridge_scan` report. */
export type BridgeScanSource = {
  agent: string;
  status: "found" | "missing" | "restricted";
  detail?: string | null;
};

export type BridgeScanReport = {
  sources: BridgeScanSource[];
};

export type BridgeEntryKind = "key-plan" | "subscription" | "reference";

/** A credential as shown in the settings page — the secret never crosses the wire. */
export type BridgeEntryView = {
  id: string;
  kind: BridgeEntryKind;
  label: string;
  preset?: string | null;
  maskedKey?: string | null;
  /** `scan:<origin>` | `manual`. */
  source: string;
  note?: string | null;
  createdAt: string;
};

/** One cross-usable model for an agent (Marionette-side catalog). */
export type BridgeModel = {
  entryId: string;
  modelId: string;
  label: string;
  preset: string;
};

/**
 * One cross-use source (credential entry) the catalog groups models by.
 * `fromScan` marks reference entries discovered by the credential scan vs
 * keys the user added manually. Sources with zero models are still listed —
 * their models are simply not enumerable (e.g. openrouter), custom ids only.
 */
export type BridgeModelSource = {
  entryId: string;
  preset: string;
  label: string;
  fromScan: boolean;
};

export type BridgeModelsResult = {
  models: BridgeModel[];
  /** One entry per usable credential; older backends omit it (derive from models). */
  sources?: BridgeModelSource[] | null;
  note: string | null;
};

/** What `bridge_bind` persists per dialog. */
export type BridgeSelection = {
  entryId: string;
  modelId: string;
};

export async function bridgeScan(): Promise<BridgeScanReport | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<BridgeScanReport>("bridge_scan");
  } catch {
    return null;
  }
}

export async function bridgeListEntries(): Promise<BridgeEntryView[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<BridgeEntryView[]>("bridge_list_entries");
  } catch {
    return [];
  }
}

export async function bridgeAddKey(
  preset: string,
  label: string | null,
  key: string,
): Promise<BridgeEntryView> {
  return invoke<BridgeEntryView>("bridge_add_key", { preset, label, key });
}

export async function bridgeDeleteEntry(id: string): Promise<void> {
  await invoke("bridge_delete_entry", { id });
}

export async function bridgeListModels(agentId: string): Promise<BridgeModelsResult> {
  if (!isTauriRuntime()) return { models: [], note: null };
  try {
    return await invoke<BridgeModelsResult>("bridge_list_models", { agentId });
  } catch {
    return { models: [], note: null };
  }
}

/**
 * Bind / unbind a cross-use model for one dialog. The backend applies the
 * binding at spawn time, so a live process only picks it up after a restart.
 */
export async function bridgeBind(
  sessionId: string,
  selection: BridgeSelection | null,
): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("bridge_bind", { sessionId, selection });
}

/**
 * Current binding for a dialog — used to restore the chip display after a
 * restart/suspend without re-deriving it from agent advertisements.
 *
 * NOTE: not in the initial M1 command contract; the backend is expected to
 * expose `bridge_get_binding`. Returns null until the command exists.
 */
export async function bridgeGetBinding(sessionId: string): Promise<BridgeSelection | null> {
  if (!isTauriRuntime() || !sessionId) return null;
  try {
    return await invoke<BridgeSelection | null>("bridge_get_binding", { sessionId });
  } catch {
    return null;
  }
}

// ─── M2 Translation Engine (翻译引擎) ────────────────────────────────────────

/**
 * Embedded magpie engine state (`engine_status` / `engine-state-changed`).
 * Tagged by `state`; `running.reused` means a user-installed magpie on 3425
 * was detected and reused rather than spawning our own subprocess.
 */
export type EngineStatus =
  | { state: "absent" }
  | { state: "downloading"; downloaded: number; total: number }
  | { state: "starting" }
  | { state: "running"; port: number; version?: string | null; reused?: boolean }
  | { state: "failed"; error: string };

/** `engine-download-progress` event payload (bytes). */
export type EngineDownloadProgress = { downloaded: number; total: number };

export async function engineStatus(): Promise<EngineStatus | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<EngineStatus>("engine_status");
  } catch {
    return null;
  }
}

/**
 * Idempotent engine enable (download → serve → provider register → running).
 * Runs in the background; resolves with the snapshot taken at kick-off and
 * the UI follows `engine-download-progress` / `engine-state-changed` events.
 */
export async function engineEnable(): Promise<EngineStatus | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<EngineStatus>("engine_enable");
  } catch {
    return null;
  }
}

/** One subscription quota window from `magpie quota` (percent-based). */
export type EngineQuotaWindow = {
  name: string;
  used?: number | null;
  remaining?: number | null;
  resetsAt?: string | null;
};

/** One subscription family's quota (merged into the Usage panel). */
export type EngineQuotaEntry = {
  provider: string;
  name: string;
  user?: string | null;
  windows: EngineQuotaWindow[];
  /** Key balance when the vendor reports money instead of windows. */
  balance?: string | null;
};

/** Subscription quotas from the engine; empty when the engine is not running. */
export async function engineQuota(force = false): Promise<EngineQuotaEntry[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<EngineQuotaEntry[]>("engine_quota", { force });
  } catch {
    return [];
  }
}

/** Manually stop the gateway (self-spawned instances only; reused installs no-op). */
export async function engineStop(): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await invoke("engine_stop");
  } catch {
    // stop is best-effort; the popover refreshes state right after anyway.
  }
}

/** Agent proxy config: single exit address injected into spawned agents. */
export async function getProxyConfig(): Promise<ProxyConfig> {
  if (!isTauriRuntime()) return { enabled: false, url: "" };
  return await invoke<ProxyConfig>("get_proxy_config");
}

/** Persist the proxy config (App restarts the live agent afterwards). */
export async function setProxyConfig(config: ProxyConfig): Promise<void> {
  await invoke("set_proxy_config", { config });
}

/** Round-trip through `url` to OpenAI's API; reports latency or failure. */
export async function testProxy(url: string): Promise<ProxyTestResult> {
  return await invoke<ProxyTestResult>("test_proxy", { url });
}

// ── Telegram 远程桥（docs/telegram-bridge.md）──────────────────────────────

/** 通知偏好（落 ~/.marionette/telegram.json）。 */
export type TelegramPrefs = {
  notifyTurn: boolean;
  notifyWait: boolean;
  notifyError: boolean;
  contentPreview: boolean;
};

/** 配对请求（bot 收到第一条私聊消息后挂起，等这里确认）。 */
export type TelegramPendingPairing = {
  username: string;
  firstName: string;
  chatId: number;
  receivedAtMs: number;
};

export type TelegramStatus = {
  configured: boolean;
  enabled: boolean;
  botUsername: string | null;
  paired: boolean;
  pairedUsername: string | null;
  pendingPairing: TelegramPendingPairing | null;
  prefs: TelegramPrefs;
  lastError: string | null;
};

export type TelegramSave = {
  enabled?: boolean;
  /** 空串 = 清除 token；不传 = 不动。 */
  token?: string;
  prefs?: TelegramPrefs;
};

/** `telegram-remote-inbound` 事件载荷（手机发来的文字 → performSend）。 */
export type TelegramInbound = {
  sessionId: string;
  text: string;
  messageId: number | null;
};

export const TELEGRAM_INBOUND_EVENT = "telegram-remote-inbound";

/** `telegram-remote-command` 事件载荷（Rust 命令面 → 前端执行 → result 回执）。 */
export type TelegramCommand =
  | {
      kind: "newSession";
      /** true = 建在聊天区（projectId 为 null）。 */
      chat: boolean;
      projectId: string | null;
      agentId: string;
      modelId?: string | null;
      modelLabel?: string | null;
      modeId?: string | null;
      modeLabel?: string | null;
    }
  | {
      kind: "setModel";
      sessionId: string;
      modelId: string;
      modelLabel?: string | null;
    };

export const TELEGRAM_COMMAND_EVENT = "telegram-remote-command";

/** 远程命令（/new、/model）执行完的回执；bindSessionId 同时切换裸文本绑定。 */
export async function telegramRemoteResult(
  text: string,
  bindSessionId?: string | null,
): Promise<void> {
  try {
    await invoke("telegram_remote_result", {
      text,
      bindSessionId: bindSessionId ?? null,
    });
  } catch {
    // 回执是尽力而为；失败只影响手机上少一条确认。
  }
}

export async function telegramStatus(): Promise<TelegramStatus | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<TelegramStatus>("telegram_remote_status");
  } catch {
    return null;
  }
}

/** 保存设置。带新 token 时后端先 getMe 校验，失败会 throw（错误给浮层显示）。 */
export async function telegramSave(config: TelegramSave): Promise<TelegramStatus> {
  return await invoke<TelegramStatus>("telegram_remote_save", { config });
}

export async function telegramAcceptPairing(): Promise<TelegramStatus> {
  return await invoke<TelegramStatus>("telegram_remote_accept_pairing");
}

export async function telegramDismissPairing(): Promise<TelegramStatus> {
  return await invoke<TelegramStatus>("telegram_remote_dismiss_pairing");
}

export async function telegramResetPairing(): Promise<TelegramStatus> {
  return await invoke<TelegramStatus>("telegram_remote_reset_pairing");
}

/** performSend 之后的回执：ok=false 带错误摘要，后端把它推回手机。 */
export async function telegramAck(
  ok: boolean,
  error: string | null,
  messageId: number | null,
  sessionId: string,
): Promise<void> {
  try {
    await invoke("telegram_remote_ack", {
      ok,
      error: error ?? null,
      messageId,
      sessionId,
    });
  } catch {
    // ack 是尽力而为；失败只影响手机上少一条 ✓。
  }
}

// ── 跨 agent 共享记忆（memory.rs） ──────────────────────────────────────

export type MemoryEntry = {
  id: string;
  source: string;
  title: string;
  text: string;
  enabled: boolean;
  createdAtMs: number;
  updatedAtMs: number;
  originPath?: string | null;
  originMtimeMs?: number | null;
};

export type MemoryStore = {
  version: number;
  enabled: boolean;
  entries: MemoryEntry[];
};

export type MemoryImportSource = {
  source: string;
  title: string;
  path: string;
  exists: boolean;
  bytes: number;
  mtimeMs: number;
  excerpt: string;
};

export type MemoryImportResult = {
  imported: string[];
  updated: string[];
  unchanged: string[];
  sources: MemoryImportSource[];
};

export async function memoryGet(): Promise<MemoryStore> {
  if (!isTauriRuntime()) return mockMemoryStore;
  return await invoke<MemoryStore>("memory_get");
}

export async function memorySetEnabled(enabled: boolean): Promise<void> {
  await invoke("memory_set_enabled", { enabled });
}

export async function memoryAdd(
  title: string,
  text: string,
  source?: string,
): Promise<MemoryEntry> {
  return await invoke<MemoryEntry>("memory_add", {
    title,
    text,
    source: source ?? null,
  });
}

export async function memoryUpdate(id: string, title: string, text: string): Promise<void> {
  await invoke("memory_update", { id, title, text });
}

export async function memoryDelete(id: string): Promise<void> {
  await invoke("memory_delete", { id });
}

export async function memorySetEntryEnabled(
  id: string,
  enabled: boolean,
): Promise<void> {
  await invoke("memory_set_entry_enabled", { id, enabled });
}

export async function memoryImportPreview(): Promise<{
  sources: MemoryImportSource[];
}> {
  return await invoke("memory_import_preview");
}

export async function memoryImportRun(): Promise<MemoryImportResult> {
  return await invoke("memory_import_run");
}

export async function memoryDistill(sessionId: string): Promise<{ draft: string }> {
  return await invoke("memory_distill", { sessionId });
}

// ── DSH 插件面板（dsh_plugins.rs） ──────────────────────────────────────

export type DshPlugin = {
  name: string;
  version: string;
};

export async function dshPluginsList(): Promise<DshPlugin[]> {
  if (!isTauriRuntime()) return mockDshPlugins;
  // 防御：后端形状异常时给空清单，不让面板整页崩掉。
  const list = await invoke<unknown>("dsh_plugins_list");
  return Array.isArray(list) ? (list as DshPlugin[]) : [];
}

export async function dshPluginAdd(spec: string): Promise<{ summary: string }> {
  return await invoke("dsh_plugin_add", { spec });
}

export async function dshPluginRemove(name: string): Promise<void> {
  await invoke("dsh_plugin_remove", { name });
}
