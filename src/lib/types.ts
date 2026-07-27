export type Project = {
  id: string;
  name: string;
  rootPath: string;
  createdAt: string;
  lastOpenedAt: string;
};

/** A CLI the ACP bridge shells out to (installed separately from the bridge). */
export type AgentDependency = {
  command: string;
  label: string;
  package: string | null;
};

/** How AgentShell can put an agent's ACP command on the machine. */
export type AgentInstallSpec = {
  manager: "npm" | "manual";
  package: string | null;
  requires: AgentDependency[];
  note: string | null;
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
  install: AgentInstallSpec;
};

/** What version of an agent CLI is on disk, and what npm has published. */
export type AgentVersionInfo = {
  id: string;
  package: string | null;
  installed: string | null;
  latest: string | null;
  updateAvailable: boolean;
  /** Why a field is empty (offline, no `--version`, manual install, …). */
  note: string | null;
};

export type AgentCommandStatus = {
  id: string;
  /** `incomplete` = bridge present but a CLI it drives is missing. */
  status: "installed" | "incomplete" | "missing" | "failed";
  path: string | null;
  message: string;
  /** AgentShell knows an npm package for whatever is missing. */
  installable?: boolean;
  /** Human labels of the missing pieces. */
  missing?: string[];
};

export type AgentInstallResult = {
  agentId: string;
  installed: string[];
  message: string;
  status: AgentCommandStatus;
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
  /**
   * Grok `/always-approve` (and similar permission auto-approve). Separate from
   * plan/build mode — restored by replaying the slash command on warm-up.
   */
  preferredAlwaysApprove?: boolean | null;
};

/** Snapshot of Composer model/mode/effort bound to a dialog. */
export type SessionComposerPrefs = {
  preferredModel?: string | null;
  preferredMode?: string | null;
  preferredEffort?: number | null;
  preferredEffortId?: string | null;
  preferredAlwaysApprove?: boolean | null;
};

export type SessionEvent =
  | {
      type: "user_message";
      sessionId: string;
      text: string;
      /** Stable anchor for outline / edit&resend. */
      messageId?: string;
      createdAt: string;
      /** Snapshot of Composer config at send time. */
      agentId?: string;
      agentLabel?: string;
      modelId?: string;
      modelLabel?: string;
      modeLabel?: string;
      effortLabel?: string;
    }
  | {
      type: "assistant_message";
      sessionId: string;
      text: string;
      messageId?: string;
      createdAt: string;
      /** Metadata inherited from preceding user_message snapshot. */
      agentId?: string;
      agentLabel?: string;
      modelId?: string;
      modelLabel?: string;
      modeLabel?: string;
      effortLabel?: string;
      /** Generation duration in ms, computed at turn completion. */
      durationMs?: number;
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
      /** Rendered card body (header + path + output) — composed from the fields below. */
      text: string;
      toolCallId?: string;
      status?: string;
      title?: string;
      /**
       * Tool name from the first `tool_call` (`task`, `read`, `bash`…).
       * Updates rename `title` to a human summary, so this is the only stable
       * way to tell *what kind* of tool is running.
       */
      toolName?: string;
      /** File the tool is working on (ACP `locations[0]`). */
      path?: string;
      /** What the tool produced (ACP `content[]` / `rawOutput`), clipped. */
      detail?: string;
      /** Clipped `rawInput`, shown only until real output arrives. */
      input?: string;
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

// ─── Project context (MCP servers + skills lent between agents) ─────────────

export type McpServerSpec = {
  id: string;
  name: string;
  transport: "stdio" | "http" | "sse";
  command: string | null;
  args: string[];
  /** Key names only — values stay in the owning agent's config. */
  envKeys: string[];
  /** HTTP header names only (e.g. Authorization) — values re-read at inject time. */
  headerKeys?: string[];
  url: string | null;
  sources: string[];
  sourcePaths: string[];
  /** Agents that already load it themselves (never injected into these). */
  agents: string[];
};

export type SkillSpec = {
  id: string;
  name: string;
  description: string;
  dir: string;
  file: string;
  sources: string[];
  agents: string[];
};

export type ContextInventory = {
  mcpServers: McpServerSpec[];
  skills: SkillSpec[];
  notes: string[];
  scannedAt: string;
};

export type ContextSelection = {
  version: number;
  mcpServers: Record<string, boolean>;
  skills: Record<string, boolean>;
  updatedAt: string;
};

export type ProjectContext = {
  projectId: string;
  inventory: ContextInventory;
  selection: ContextSelection;
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

/**
 * ACP `available_commands_update` entry — slash commands sent as normal prompts.
 * @see https://agentclientprotocol.com/protocol/v1/slash-commands
 */
export type AvailableCommand = {
  name: string;
  description: string;
  input?: { hint: string };
};

export type ProviderInfo = {
  provider: string;
  label: string;
  hasKey: boolean;
  /**
   * `oauth` entries come from `opencode auth login` and hold a refresh token
   * this app cannot re-create — overwriting or deleting one needs confirmation.
   */
  authKind: "api" | "oauth" | "unknown";
};
