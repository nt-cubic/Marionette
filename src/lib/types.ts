export type Project = {
  id: string;
  name: string;
  rootPath: string;
  createdAt: string;
  lastOpenedAt: string;
};

export type AgentConfig = {
  id: string;
  label: string;
  command: string;
  args: string[];
  cwdMode: "project-root" | "custom";
  customCwd?: string;
  launchMode: "pty" | "server" | "hybrid";
  sendStrategy: "stdin" | "bracketed-paste" | "http";
  parser: "ansi-raw" | "opencode-sse" | "none";
  transport: "pty" | "acp";
  enabled: boolean;
};

export type AgentCommandStatus = {
  id: string;
  status: "installed" | "missing" | "failed";
  path: string | null;
  message: string;
};

export type TerminalOutput = {
  sessionId: string;
  data: string;
  exited: boolean;
  error: string | null;
};

export type AcpEvent = {
  sessionId: string;
  kind: "notification" | "request" | "response" | "stderr" | "system";
  method: string | null;
  data: unknown;
};

export type SessionStatus = "starting" | "running" | "waiting" | "exited" | "error";

export type SessionViewMode = "clean" | "raw-terminal" | "diff" | "logs";

export type Session = {
  id: string;
  projectId: string;
  agentId: string;
  label: string;
  cwd: string;
  status: SessionStatus;
  processId: number | null;
  ptyId: string | null;
  startedAt: string;
  lastActiveAt: string;
  exitedAt?: string;
  exitCode?: number;
  rawLogPath: string;
  transcriptPath: string;
  handoffPath: string;
  viewMode: SessionViewMode;
};

export type SessionEvent =
  | {
      type: "user_message";
      sessionId: string;
      text: string;
      createdAt: string;
    }
  | {
      type: "assistant_message";
      sessionId: string;
      text: string;
      createdAt: string;
    }
  | {
      type: "raw_chunk";
      sessionId: string;
      text: string;
      createdAt: string;
    }
  | {
      type: "handoff_prepared";
      sessionId: string;
      targetAgentId: string;
      handoffPath: string;
      prompt: string;
      createdAt: string;
    }
  | {
      type: "file_change";
      sessionId: string;
      path: string;
      changeType: "added" | "modified" | "deleted";
      createdAt: string;
    };

export type ChangedFile = {
  path: string;
  changeType: "added" | "modified" | "deleted" | "untracked";
};

export type HandoffResult = {
  projectId: string;
  targetAgentId: string;
  handoffPath: string;
  prompt: string;
  createdAt: string;
};

export type UsageWindow = {
  id: string;
  label: string;
  percentage: number | null;
};

export type UsageSnapshot = {
  agentId: string;
  agentLabel: string;
  windows: UsageWindow[];
  refreshedAt: string;
};

// ─── ACP Capability Types ───────────────────────────────────────────────────

export type ModeDef = {
  id: string;
  label: string;
};

export type ModelDef = {
  id: string;
  label: string;
};

export type ThinkingEffort = {
  min: number;
  max: number;
  default: number;
};

export type CapabilitySnapshot = {
  modes: ModeDef[];
  models: ModelDef[];
  thinkingEffort: ThinkingEffort | null;
  supportsCancel: boolean;
  currentMode: string | null;
  currentModel: string | null;
  currentEffort: number | null;
  modelConfigId: string | null;
  modeConfigId: string | null;
  effortConfigId: string | null;
};
