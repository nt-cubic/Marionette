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
  kind: "notification" | "request" | "response" | "stderr" | "system" | "error";
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
  /** Last model id for this dialog — restored when agent still advertises it. */
  preferredModel?: string | null;
  /** Last mode id (build / plan / …). */
  preferredMode?: string | null;
  /** Numeric effort 0–1 when agent uses a slider. */
  preferredEffort?: number | null;
  /** Discrete effort id (e.g. low / high) when agent uses select options. */
  preferredEffortId?: string | null;
};

/** Snapshot of Composer model/mode/effort bound to a dialog. */
export type SessionComposerPrefs = {
  preferredModel?: string | null;
  preferredMode?: string | null;
  preferredEffort?: number | null;
  preferredEffortId?: string | null;
};

export type SessionEvent =
  | {
      type: "user_message";
      sessionId: string;
      text: string;
      /** Stable anchor for outline / edit&resend. */
      messageId?: string;
      createdAt: string;
    }
  | {
      type: "assistant_message";
      sessionId: string;
      text: string;
      messageId?: string;
      createdAt: string;
    }
  | {
      type: "thought";
      sessionId: string;
      text: string;
      messageId?: string;
      createdAt: string;
    }
  | {
      type: "tool_call";
      sessionId: string;
      text: string;
      toolCallId?: string;
      status?: string;
      title?: string;
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
  summary?: string;
};

export type UsageWindow = {
  id: string;
  label: string;
  /** Used percentage 0–100 when known (context fill or rate-limit used). */
  percentage: number | null;
  /** Extra line: token counts, % left, reset hint, formatted cost, etc. */
  detail?: string | null;
  kind?: "context" | "rate_limit" | "cost" | "provider";
};

export type UsageSnapshot = {
  agentId: string;
  agentLabel: string;
  windows: UsageWindow[];
  refreshedAt: string;
  /** Human-readable footnote about data freshness / source. */
  note?: string | null;
  cost?: { amount: number; currency: string } | null;
};

// ─── ACP Capability Types ───────────────────────────────────────────────────

export type ModeDef = {
  id: string;
  label: string;
};

export type ModelDef = {
  id: string;
  label: string;
  /** ACP option description (version, pricing, context) — never invent this. */
  description?: string | null;
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
  /**
   * Discrete effort/thought levels from ACP (Claude: default/low/high/max).
   * When non-empty, Composer should use these strings — not the 0–1 slider.
   */
  effortOptions: ModeDef[];
  supportsCancel: boolean;
  currentMode: string | null;
  currentModel: string | null;
  currentEffort: number | null;
  /** String effort id when agent uses select options (e.g. "high"). */
  currentEffortId: string | null;
  modelConfigId: string | null;
  modeConfigId: string | null;
  effortConfigId: string | null;
};
