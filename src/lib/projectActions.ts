import { invoke } from "@tauri-apps/api/core";
import {
  isTauriRuntime,
  killAcpTerminal,
  listAcpTerminals,
  listAcpTerminalOutputs,
} from "./api";

/**
 * User-owned project actions (P1-14).
 *
 * The commands are stored by the Rust side under
 * `<project>/.marionette/project-actions.json` and run through the shared ACP
 * terminal host. Nothing here reads a repository's package.json/Makefile: the
 * list is exactly what the user saved in Marionette.
 */
export type ProjectActionKind = "setup" | "dev" | "test" | "cleanup";

export type ProjectAction = {
  id: string;
  kind: ProjectActionKind;
  label: string;
  command: string;
  /** Relative to the project root; absolute paths and `..` are rejected. */
  cwd?: string | null;
  /** The exact command the user confirmed before the first run. Machine-local
   *  (`~/.marionette/project-action-confirmations.json`), never stored in the
   *  project tree. */
  confirmedCommand?: string | null;
  /** Setup only: opt in to running after a worktree is created. */
  runAfterWorktree: boolean;
};

export type ProjectActionRunStatus =
  | "confirmation_required"
  | "started"
  | "running"
  | "completed"
  | "failed";

export type ProjectActionRunResult = {
  status: ProjectActionRunStatus;
  command: string;
  terminalId?: string;
  exitCode?: number;
  signal?: string;
  /** Tail of the terminal output (the backend caps it to ~32 KB). */
  output?: string;
  truncated: boolean;
  error?: string;
};

export type ProjectActionStarted = {
  actionId: string;
  label: string;
  terminalId: string;
};

export type WorktreeSetupFailure = {
  actionId: string;
  label: string;
  error: string;
};

export type WorktreeSetupReport = {
  started: ProjectActionStarted[];
  failed: WorktreeSetupFailure[];
  waited: boolean;
  ok: boolean;
};

export async function listProjectActions(projectId: string): Promise<ProjectAction[]> {
  if (!isTauriRuntime()) return [];
  return invoke<ProjectAction[]>("project_actions_list", { projectId });
}

/**
 * Save the full list. The backend returns the normalized list (trimmed
 * fields, forward-slash cwd, non-setup `runAfterWorktree` cleared). A command
 * change clears its first-run confirmation; a save never mints one.
 */
export async function saveProjectActions(
  projectId: string,
  actions: ProjectAction[],
): Promise<ProjectAction[]> {
  if (!isTauriRuntime()) throw new Error("Project actions are available in the desktop app");
  return invoke<ProjectAction[]>("project_actions_save", { projectId, actions });
}

/**
 * Run one saved action. `confirm` is required the first time (or after the
 * command changed); without it the backend returns `confirmation_required`
 * and does not spawn anything.
 *
 * `wait` is only meaningful for setup: when false (the default) the call
 * returns as soon as the terminal starts, so a failing setup never blocks
 * something else unless the caller explicitly chose to wait.
 */
export async function runProjectAction(
  projectId: string,
  actionId: string,
  options: { confirm?: boolean; wait?: boolean; waitForMs?: number | null } = {},
): Promise<ProjectActionRunResult> {
  if (!isTauriRuntime()) throw new Error("Project actions are available in the desktop app");
  return invoke<ProjectActionRunResult>("project_actions_run", {
    projectId,
    actionId,
    confirm: options.confirm ?? false,
    wait: options.wait ?? false,
    waitForMs: options.waitForMs ?? null,
  });
}

/**
 * Worktree hook entry point for future worktree creation. Only setup actions
 * with `runAfterWorktree` and an already-confirmed exact command run. The
 * action list is read from the original project, not the worktree checkout.
 */
export async function runProjectWorktreeSetup(
  projectId: string,
  worktreeRoot: string,
  options: { wait?: boolean; waitForMs?: number | null } = {},
): Promise<WorktreeSetupReport> {
  if (!isTauriRuntime()) throw new Error("Project actions are available in the desktop app");
  return invoke<WorktreeSetupReport>("project_actions_run_worktree_setup", {
    projectId,
    worktreeRoot,
    wait: options.wait ?? false,
    waitForMs: options.waitForMs ?? null,
  });
}

export type ProjectActionTerminalSnapshot = {
  running: boolean;
  exitCode?: number;
  signal?: string;
  output: string;
  command?: string;
};

/**
 * Read an action terminal through the existing background-task APIs. Poll
 * while `running` is true; stop with {@link stopProjectAction}.
 */
export async function readProjectActionTerminal(
  terminalId: string,
): Promise<ProjectActionTerminalSnapshot | null> {
  if (!isTauriRuntime()) return null;
  const [rows, outputs] = await Promise.all([listAcpTerminals(), listAcpTerminalOutputs()]);
  const row = rows.find((entry) => entry.terminalId === terminalId);
  if (!row) return null;
  const output = outputs.find((entry) => entry.terminalId === terminalId)?.output;
  return {
    running: row.running,
    exitCode: row.exitStatus?.exitCode,
    signal: row.exitStatus?.signal,
    output: output ?? row.outputTail ?? "",
    command: row.command,
  };
}

/** Stop an action terminal. Uses the same kill path as the background panel. */
export async function stopProjectAction(terminalId: string): Promise<void> {
  await killAcpTerminal(terminalId);
}

export function actionNeedsConfirmation(action: ProjectAction): boolean {
  return (action.confirmedCommand ?? "").trim() !== action.command.trim();
}
