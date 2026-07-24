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
    enabled: true
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
    enabled: true
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
    enabled: true
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
    enabled: true
  }
];

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
