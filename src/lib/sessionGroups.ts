/**
 * Shelf grouping for Chat-store dialogs (2026-10-26).
 *
 * Chat sessions with `origin: "memo"` are conversations grown out of the
 * 收纳盒 ("开新对话" on picked entries). They are ordinary Chat sessions for
 * every runtime path (spawn cwd, prefs, tabs) — only the shelf splits them
 * into their own group below 聊天, mirroring karakeep's "collected →
 * organized" flow.
 */

import { CHAT_PROJECT_ID } from "./api";
import type { Project, Session } from "./types";

export type ShelfChatSplit = {
  staff: Session[];
  chat: Session[];
  memo: Session[];
  workers: Session[];
};

/**
 * Worker conversations are an implementation detail of a parent task. They
 * can still be opened from the task card or background row, but they must not
 * occupy the user's normal thread list (T3 Code hides subagent threads the
 * same way).
 */
export function isDelegatedWorkerSession(
  session: Pick<Session, "origin" | "parentSessionId" | "lineage">,
): boolean {
  return (
    session.origin === "agent-worker" ||
    session.origin === "chief-worker" ||
    Boolean(session.parentSessionId) ||
    session.lineage?.relationship === "subagent"
  );
}

/** Split the virtual Chat project's sessions from hidden worker subtrees. */
export function splitShelfChatSessions(sessions: ReadonlyArray<Session>): ShelfChatSplit {
  const staff: Session[] = [];
  const chat: Session[] = [];
  const memo: Session[] = [];
  const workers: Session[] = [];
  for (const session of sessions) {
    if (session.projectId !== CHAT_PROJECT_ID) continue;
    if (isDelegatedWorkerSession(session)) workers.push(session);
    else if (session.origin === "chief") staff.push(session);
    else if (session.origin === "memo") memo.push(session);
    else chat.push(session);
  }
  return { staff, chat, memo, workers };
}

/**
 * Ghost project left behind by the abandoned 2026-10-22 orb-assistant
 * "capture inbox" scheme: a project whose root lives under Marionette's own
 * data dir (`~/.marionette/inbox`). Nothing creates it anymore; hide it so
 * users stop seeing an empty "inbox / No sessions" row.
 */
export function isLegacyGhostProject(project: Project): boolean {
  const root = project.rootPath.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
  return root.endsWith(".marionette/inbox");
}

/** Projects list minus ghost rows (defensive at every load site). */
export function withoutGhostProjects(projects: ReadonlyArray<Project>): Project[] {
  return projects.filter((project) => !isLegacyGhostProject(project));
}
