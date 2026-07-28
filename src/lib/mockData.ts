import type { AgentConfig, ChangedFile, Project, Session, SessionEvent } from "./types";

export const projects: Project[] = [
  {
    id: "project-marionette",
    name: "Marionette",
    rootPath: "D:\\Project\\Marionette",
    createdAt: "2026-07-09T00:00:00.000Z",
    lastOpenedAt: "2026-07-09T09:15:00.000Z"
  },
  {
    id: "project-client-tools",
    name: "ClientTools",
    rootPath: "D:\\Work\\ClientTools",
    createdAt: "2026-07-08T08:00:00.000Z",
    lastOpenedAt: "2026-07-08T18:30:00.000Z"
  }
];

/** Browser mock only — the desktop app gets install specs from Rust. */
const mockInstall = (pkg: string | null): AgentConfig["install"] =>
  pkg
    ? { manager: "npm", package: pkg, requires: [], note: null }
    : { manager: "manual", package: null, requires: [], note: "Vendor installer only." };

export const agents: AgentConfig[] = [
  {
    id: "opencode",
    label: "OpenCode",
    command: "opencode",
    args: ["acp"],
    cwdMode: "project-root",
    launchMode: "pty",
    sendStrategy: "stdin",
    parser: "ansi-raw",
    transport: "acp",
    enabled: true,
    install: mockInstall("opencode-ai")
  },
  {
    id: "codex",
    label: "Codex CLI",
    command: "codex-acp",
    args: [],
    cwdMode: "project-root",
    launchMode: "pty",
    sendStrategy: "stdin",
    parser: "ansi-raw",
    transport: "acp",
    enabled: true,
    install: mockInstall("@agentclientprotocol/codex-acp")
  },
  {
    id: "claude-code",
    label: "Claude Code",
    command: "claude-agent-acp",
    args: [],
    cwdMode: "project-root",
    launchMode: "pty",
    sendStrategy: "stdin",
    parser: "ansi-raw",
    transport: "acp",
    enabled: true,
    install: mockInstall("@agentclientprotocol/claude-agent-acp")
  },
  {
    id: "grok-build",
    label: "Grok Build",
    command: "grok",
    args: ["agent", "stdio"],
    cwdMode: "project-root",
    launchMode: "pty",
    sendStrategy: "stdin",
    parser: "ansi-raw",
    transport: "acp",
    enabled: true,
    install: mockInstall(null)
  }
];

/** Browser mock for the Project context card (desktop scans the real machine). */
export const projectContext: import("./types").ProjectContext = {
  projectId: "project-marionette",
  inventory: {
    mcpServers: [
      {
        id: "blender",
        name: "blender",
        transport: "stdio",
        command: "cmd",
        args: ["/c", "uvx", "blender-mcp"],
        envKeys: [],
        url: null,
        sources: ["opencode", "codex"],
        sourcePaths: ["~/.config/opencode/opencode.jsonc", "~/.codex/config.toml"],
        agents: ["opencode", "codex"]
      },
      {
        id: "ai-game-developer",
        name: "ai-game-developer",
        transport: "http",
        command: null,
        args: [],
        envKeys: [],
        url: "https://ai-game.dev/mcp",
        sources: ["codex"],
        sourcePaths: ["~/.codex/config.toml"],
        agents: ["codex"]
      }
    ],
    skills: [
      {
        id: "unity-tools",
        name: "unity-tools",
        description: "Unity project helpers",
        dir: ".marionette/skills/unity-tools",
        file: ".marionette/skills/unity-tools/SKILL.md",
        sources: ["project"],
        agents: []
      },
      {
        id: "docx",
        name: "docx",
        description: "Word documents",
        dir: "~/.config/opencode/skills/docx",
        file: "~/.config/opencode/skills/docx/SKILL.md",
        sources: ["opencode", "grok-build"],
        agents: ["opencode", "grok-build"]
      }
    ],
    notes: [],
    scannedAt: "0"
  },
  selection: { version: 1, mcpServers: {}, skills: {}, updatedAt: "0" }
};

export const sessions: Session[] = [
  {
    id: "session-codex-1",
    projectId: "project-marionette",
    agentId: "codex",
    label: "Codex CLI / layout pass",
    cwd: "D:\\Project\\Marionette",
    status: "running",
    processId: 18424,
    startedAt: "2026-07-09T09:09:00.000Z",
    lastActiveAt: "2026-07-09T09:18:00.000Z",
    transcriptPath: "D:\\Project\\Marionette\\.marionette\\transcripts\\codex-20260709.jsonl",
    handoffPath: "D:\\Project\\Marionette\\.marionette\\handoff.md",
    viewMode: "clean"
  },
  {
    id: "session-claude-1",
    projectId: "project-marionette",
    agentId: "claude-code",
    label: "Claude Code / review notes",
    cwd: "D:\\Project\\Marionette",
    status: "waiting",
    processId: 12964,
    startedAt: "2026-07-09T08:44:00.000Z",
    lastActiveAt: "2026-07-09T09:02:00.000Z",
    transcriptPath: "D:\\Project\\Marionette\\.marionette\\transcripts\\claude-20260709.jsonl",
    handoffPath: "D:\\Project\\Marionette\\.marionette\\handoff.md",
    viewMode: "clean"
  },
  {
    id: "session-opencode-1",
    projectId: "project-marionette",
    agentId: "opencode",
    label: "OpenCode / idle",
    cwd: "D:\\Project\\Marionette",
    status: "exited",
    processId: null,
    startedAt: "2026-07-08T21:12:00.000Z",
    lastActiveAt: "2026-07-08T21:28:00.000Z",
    exitedAt: "2026-07-08T21:28:00.000Z",
    exitCode: 0,
    transcriptPath: "D:\\Project\\Marionette\\.marionette\\transcripts\\opencode-20260708.jsonl",
    handoffPath: "D:\\Project\\Marionette\\.marionette\\handoff.md",
    viewMode: "clean"
  }
];

export const changedFiles: ChangedFile[] = [
  { path: "docs/03-implementation-guide.md", changeType: "modified" },
  { path: "src/app/App.tsx", changeType: "added" },
  { path: "src/components/SessionView.tsx", changeType: "added" }
];

export const sessionEvents: SessionEvent[] = [
  {
    type: "user_message",
    sessionId: "session-codex-1",
    text: "Start Milestone 1 and keep it to a desktop shell only.",
    createdAt: "2026-07-09T09:10:00.000Z"
  },
  {
    type: "handoff_prepared",
    sessionId: "session-codex-1",
    targetAgentId: "claude-code",
    handoffPath: "D:\\Project\\Marionette\\.marionette\\handoff.md",
    prompt: "Continue from the static shell and inspect M2 scope before adding backend commands.",
    createdAt: "2026-07-09T09:14:00.000Z"
  }
];

