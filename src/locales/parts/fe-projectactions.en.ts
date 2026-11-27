import type { zh } from "./fe-projectactions.zh";

/** English mirror of the project-actions fragment. */
export const en = {
  "projectActions.title": "Project actions",
  "projectActions.lead":
    "Setup, dev, test and cleanup commands live in the project's own .marionette metadata. Scripts checked into the repository are never read or run automatically.",
  "projectActions.empty": "No actions yet. Add one and write the command this project should run.",
  "projectActions.add": "New action",
  "projectActions.untitled": "Untitled action",
  "projectActions.needsConfirm": "First run needs confirmation",
  "projectActions.kind": "Kind",
  "projectActions.kindSetup": "Setup",
  "projectActions.kindDev": "Dev",
  "projectActions.kindTest": "Test",
  "projectActions.kindCleanup": "Cleanup",
  "projectActions.label": "Name",
  "projectActions.labelPlaceholder": "e.g. Install dependencies",
  "projectActions.command": "Command",
  "projectActions.commandPlaceholder": "e.g. npm install",
  "projectActions.cwd": "Working directory",
  "projectActions.cwdPlaceholder": "Relative to the project root, e.g. frontend (optional)",
  "projectActions.runAfterWorktree": "Run after a worktree is created",
  "projectActions.runAfterWorktreeHint":
    "Setup actions only; the exact command still needs one manual confirmation first.",
  "projectActions.run": "Run",
  "projectActions.waitForSetup": "Wait for it before continuing",
  "projectActions.waitHint":
    "Setup actions only. A failure never blocks opening the conversation unless you choose to wait.",
  "projectActions.confirmTitle": "Confirm the first run",
  "projectActions.confirmBody":
    "This exact command will run with your Windows account. It is stored only in Marionette's project metadata and never committed to the repository.",
  "projectActions.confirmRun": "Confirm and run",
  "projectActions.output": "Output",
  "projectActions.noOutput": "No output yet.",
  "projectActions.stop": "Stop",
  "projectActions.exitCode": "Exit code {code}",
  "projectActions.statusRunning": "Running",
  "projectActions.statusCompleted": "Completed",
  "projectActions.statusFailed": "Failed",
  "projectActions.statusStarted": "Started",
  "projectActions.statusStarting": "Starting",
  "projectActions.statusConfirmationRequired": "Waiting for confirmation",
  "projectActions.deleteTitle": "Delete action",
  "projectActions.deleteBody": "Only removes it from project metadata; repository files are untouched.",
  "projectActions.saved": "Saved",
  "projectActions.securityNote":
    "Actions are started by you through Marionette's terminal host; they never inherit an agent's permission mode or auto-approve setting.",
  "projectActions.noProject": "Project actions are available for real projects, not temporary chats.",
  "projectActions.saveFirst": "Save the action before running it.",
  "projectActions.nameRequired": "The action name cannot be empty.",
  "projectActions.commandRequired": "The command cannot be empty.",
  "projectActions.runFailed": "Run failed: {error}",
} satisfies Record<keyof typeof zh, string>;
