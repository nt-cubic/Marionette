/**
 * Park-control harness: mounts the real ProjectShelf with chats in every status
 * so headless Edge can check that the ⏸ control is always drawn, says why it
 * refuses, and only fires when the dialog really holds a process.
 */
import { createRoot } from "react-dom/client";
import { ProjectShelf } from "../src/components/ProjectShelf";
import type { Session } from "../src/lib/types";

declare global {
  interface Window {
    __log: Array<{ kind: string; id: string }>;
    __ready?: boolean;
  }
}

const session = (id: string, label: string, status: Session["status"]): Session => ({
  id,
  projectId: "chats",
  agentId: "grok-build",
  label,
  labelSource: "user",
  cwd: "D:\\Projects\\Jingzhe",
  status,
  processId: status === "exited" ? null : 4242,
  startedAt: "2026-10-08T13:00:00.000Z",
  lastActiveAt: "2026-10-08T13:30:00.000Z",
  transcriptPath: "C:\\Users\\NT's Station\\.marionette\\chats\\transcripts\\x.jsonl",
  handoffPath: "",
  viewMode: "clean",
});

const fixtures = [
  session("s-waiting", "warm and idle", "waiting"),
  session("s-running", "working right now", "running"),
  session("s-error", "last turn failed", "error"),
  session("s-exited", "already parked", "exited"),
];

window.__log = [];

createRoot(document.getElementById("root")!).render(
  <ProjectShelf
    agents={[]}
    projects={[]}
    sessions={[]}
    currentProjectId="chats"
    currentSessionId={null}
    collapsed={false}
    theme="dark"
    onCollapse={() => undefined}
    onExpand={() => undefined}
    onToggleTheme={() => undefined}
    onAddProject={() => undefined}
    onNewSession={() => undefined}
    onProjectSelect={() => undefined}
    onSessionSelect={() => undefined}
    onDeleteSession={() => undefined}
    onDeleteProject={() => undefined}
    onRenameSession={() => undefined}
    onSuspendSession={(id) => window.__log.push({ kind: "suspend", id })}
    onToggleSessionPin={() => undefined}
    chatSessions={fixtures}
    onChatSessionSelect={(s) => window.__log.push({ kind: "select", id: s.id })}
    onNewChat={() => undefined}
  />,
);

// One frame for React to commit, then the runner takes over.
requestAnimationFrame(() => requestAnimationFrame(() => (window.__ready = true)));
