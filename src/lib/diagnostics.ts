import type { AgentVersionInfo, Session } from "./types";

/**
 * Stable UI snapshot embedded in user-exported diagnostic reports.
 *
 * Keep this deliberately small and free of conversation text, project names,
 * cwd paths, prompts, and provider keys. Backend report generation adds the
 * app/OS/runtime facts and performs a second redaction pass over this object.
 */
export type DiagnosticUiState = {
  schemaVersion: 1;
  capturedAt: string;
  surfaceKind: "main" | "detached";
  page: string;
  dockMode: "floating" | "docked-hidden" | "docked-expanded";
  splitPanes: number;
  openTabs: number;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  client: {
    locale: string;
    theme: string;
    timezone: string;
    timezoneOffsetMinutes: number;
    language: string;
    userAgent: string;
    viewportWidth: number;
    viewportHeight: number;
    devicePixelRatio: number;
    visibilityState: string;
  };
  activeSession: null | {
    id: string;
    status: Session["status"];
    processId: number | null;
    startedAt: string;
    lastActiveAt: string;
    exitedAt: string | null;
    exitCode: number | null;
    origin: string;
    projectKind: "chat" | "project";
    parentSession: boolean;
    nativeSessionCount: number;
    agent: {
      id: string;
      label: string;
    };
    modelId: string | null;
    modeId: string | null;
    effortId: string | null;
    alwaysApprove: boolean;
  };
  sessions: {
    total: number;
    starting: number;
    running: number;
    waiting: number;
    error: number;
    exited: number;
  };
  agentVersions: Array<{
    id: string;
    package: string | null;
    installed: string | null;
    latest: string | null;
    updateAvailable: boolean;
    note: string | null;
  }>;
  lastVisibleError: null | {
    source: string;
    message: string;
    capturedAt?: string;
  };
  lastClientError: null | {
    source: string;
    message: string;
    capturedAt: string;
  };
};

type BuildDiagnosticUiStateInput = {
  surfaceKind: "main" | "detached";
  page: string;
  dockMode: DiagnosticUiState["dockMode"];
  splitPanes: number;
  openTabs: number;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  locale: string;
  theme: string;
  sessions: readonly Session[];
  activeSession: Session | null;
  activeAgentLabel: string;
  activeModelId: string | null;
  activeModeId: string | null;
  activeEffortId: string | null;
  agentVersions: Record<string, AgentVersionInfo>;
  lastVisibleError: DiagnosticUiState["lastVisibleError"];
  lastClientError: DiagnosticUiState["lastClientError"];
};

function browserClientFacts(): DiagnosticUiState["client"] {
  const now = new Date();
  const userAgent =
    typeof navigator === "undefined" ? "unavailable" : navigator.userAgent;
  const language =
    typeof navigator === "undefined" ? "unavailable" : navigator.language;
  const visibilityState =
    typeof document === "undefined" ? "unavailable" : document.visibilityState;
  return {
    locale: "unavailable",
    theme: "unavailable",
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "unknown",
    timezoneOffsetMinutes: -now.getTimezoneOffset(),
    language,
    userAgent,
    viewportWidth: typeof window === "undefined" ? 0 : window.innerWidth,
    viewportHeight: typeof window === "undefined" ? 0 : window.innerHeight,
    devicePixelRatio: typeof window === "undefined" ? 0 : window.devicePixelRatio,
    visibilityState,
  };
}

export function buildDiagnosticUiState(
  input: BuildDiagnosticUiStateInput,
): DiagnosticUiState {
  const sessionCounts: DiagnosticUiState["sessions"] = {
    total: input.sessions.length,
    starting: 0,
    running: 0,
    waiting: 0,
    error: 0,
    exited: 0,
  };
  for (const session of input.sessions) {
    sessionCounts[session.status] += 1;
  }

  const active = input.activeSession;
  const client = browserClientFacts();
  client.locale = input.locale;
  client.theme = input.theme;

  return {
    schemaVersion: 1,
    capturedAt: new Date().toISOString(),
    surfaceKind: input.surfaceKind,
    page: input.page,
    dockMode: input.dockMode,
    splitPanes: input.splitPanes,
    openTabs: input.openTabs,
    leftCollapsed: input.leftCollapsed,
    rightCollapsed: input.rightCollapsed,
    client,
    activeSession: active
      ? {
          id: active.id,
          status: active.status,
          processId: active.processId,
          startedAt: active.startedAt,
          lastActiveAt: active.lastActiveAt,
          exitedAt: active.exitedAt || null,
          exitCode: active.exitCode ?? null,
          origin: active.origin || "unknown",
          projectKind:
            active.projectId === "project-chat" ? "chat" : "project",
          parentSession: Boolean(active.parentSessionId),
          nativeSessionCount: active.nativeSessionIds?.length ?? 0,
          agent: {
            id: active.agentId,
            label: input.activeAgentLabel,
          },
          modelId: input.activeModelId || active.preferredModel || null,
          modeId: input.activeModeId || active.preferredMode || null,
          effortId: input.activeEffortId || active.preferredEffortId || null,
          alwaysApprove: active.preferredAlwaysApprove === true,
        }
      : null,
    sessions: sessionCounts,
    agentVersions: Object.values(input.agentVersions)
      .map((version) => ({
        id: version.id,
        package: version.package,
        installed: version.installed,
        latest: version.latest,
        updateAvailable: version.updateAvailable,
        note: version.note,
      }))
      .sort((left, right) => left.id.localeCompare(right.id)),
    lastVisibleError: input.lastVisibleError,
    lastClientError: input.lastClientError,
  };
}
