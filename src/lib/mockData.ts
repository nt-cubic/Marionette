import type { AgentConfig, ChangedFile, Project, Session, SessionEvent } from "./types";

export const projects: Project[] = [
  {
    id: "project-agentshell",
    name: "AgentShell",
    rootPath: "D:\\Project\\AgentsShell",
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
  projectId: "project-agentshell",
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
        dir: ".agentshell/skills/unity-tools",
        file: ".agentshell/skills/unity-tools/SKILL.md",
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
    projectId: "project-agentshell",
    agentId: "codex",
    label: "Codex CLI / layout pass",
    cwd: "D:\\Project\\AgentsShell",
    status: "running",
    processId: 18424,
    ptyId: "pty-codex-1",
    startedAt: "2026-07-09T09:09:00.000Z",
    lastActiveAt: "2026-07-09T09:18:00.000Z",
    rawLogPath: "D:\\Project\\AgentsShell\\.agentshell\\sessions\\codex-20260709.raw.log",
    transcriptPath: "D:\\Project\\AgentsShell\\.agentshell\\transcripts\\codex-20260709.jsonl",
    handoffPath: "D:\\Project\\AgentsShell\\.agentshell\\handoff.md",
    viewMode: "clean"
  },
  {
    id: "session-claude-1",
    projectId: "project-agentshell",
    agentId: "claude-code",
    label: "Claude Code / review notes",
    cwd: "D:\\Project\\AgentsShell",
    status: "waiting",
    processId: 12964,
    ptyId: "pty-claude-1",
    startedAt: "2026-07-09T08:44:00.000Z",
    lastActiveAt: "2026-07-09T09:02:00.000Z",
    rawLogPath: "D:\\Project\\AgentsShell\\.agentshell\\sessions\\claude-20260709.raw.log",
    transcriptPath: "D:\\Project\\AgentsShell\\.agentshell\\transcripts\\claude-20260709.jsonl",
    handoffPath: "D:\\Project\\AgentsShell\\.agentshell\\handoff.md",
    viewMode: "clean"
  },
  {
    id: "session-opencode-1",
    projectId: "project-agentshell",
    agentId: "opencode",
    label: "OpenCode / idle",
    cwd: "D:\\Project\\AgentsShell",
    status: "exited",
    processId: null,
    ptyId: null,
    startedAt: "2026-07-08T21:12:00.000Z",
    lastActiveAt: "2026-07-08T21:28:00.000Z",
    exitedAt: "2026-07-08T21:28:00.000Z",
    exitCode: 0,
    rawLogPath: "D:\\Project\\AgentsShell\\.agentshell\\sessions\\opencode-20260708.raw.log",
    transcriptPath: "D:\\Project\\AgentsShell\\.agentshell\\transcripts\\opencode-20260708.jsonl",
    handoffPath: "D:\\Project\\AgentsShell\\.agentshell\\handoff.md",
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
    type: "raw_chunk",
    sessionId: "session-codex-1",
    text: "Scaffold detected. Preparing React components and static workspace layout.",
    createdAt: "2026-07-09T09:11:00.000Z"
  },
  {
    type: "handoff_prepared",
    sessionId: "session-codex-1",
    targetAgentId: "claude-code",
    handoffPath: "D:\\Project\\AgentsShell\\.agentshell\\handoff.md",
    prompt: "Continue from the static shell and inspect M2 scope before adding backend commands.",
    createdAt: "2026-07-09T09:14:00.000Z"
  }
];
