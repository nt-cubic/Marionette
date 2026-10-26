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

export type ShelfChatSplit = { chat: Session[]; memo: Session[] };

/** Split the virtual Chat project's sessions into the two shelf groups. */
export function splitShelfChatSessions(sessions: ReadonlyArray<Session>): ShelfChatSplit {
  const chat: Session[] = [];
  const memo: Session[] = [];
  for (const session of sessions) {
    if (session.projectId !== CHAT_PROJECT_ID) continue;
    (session.origin === "memo" ? memo : chat).push(session);
  }
  return { chat, memo };
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
