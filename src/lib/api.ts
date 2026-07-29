import { invoke } from "@tauri-apps/api/core";
import { agents as mockAgents, projects as mockProjects, sessions as mockSessions } from "./mockData";
import type { AgentConfig, AgentCommandStatus, CapabilitySnapshot, Project, ProviderInfo, Session } from "./types";

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

export async function updateSessionLabel(sessionId: string, label: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("update_session_label", { sessionId, label });
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
  raw?: unknown;
};

export async function probeAgentAuth(agentId: string): Promise<AgentAuthProbe | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<AgentAuthProbe>("probe_agent_auth", { agentId });
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

/** Probe provider balance for an OpenCode-style `provider/model` id (uses local OpenCode auth.json). */
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

export async function getFileDiff(projectId: string, path: string): Promise<string> {
  if (!isTauriRuntime()) return "";
  try {
    return await invoke<string>("get_file_diff", { projectId, path });
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

export async function respondAcpPermission(requestId: string, optionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("respond_acp_permission", { requestId, optionId });
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
