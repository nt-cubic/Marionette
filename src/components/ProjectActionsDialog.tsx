import { useEffect, useMemo, useState } from "react";
import { Plus, Square, X } from "lucide-react";
import { ConfirmDialog } from "./ConfirmDialog";
import { DialogShell } from "./DialogShell";
import { useT, type DictKey } from "../lib/i18n";
import {
  actionNeedsConfirmation,
  listProjectActions,
  readProjectActionTerminal,
  runProjectAction,
  saveProjectActions,
  stopProjectAction,
  type ProjectAction,
  type ProjectActionKind,
  type ProjectActionRunStatus,
} from "../lib/projectActions";

const KIND_OPTIONS: { kind: ProjectActionKind; key: DictKey }[] = [
  { kind: "setup", key: "projectActions.kindSetup" },
  { kind: "dev", key: "projectActions.kindDev" },
  { kind: "test", key: "projectActions.kindTest" },
  { kind: "cleanup", key: "projectActions.kindCleanup" },
];

const KIND_KEY: Record<ProjectActionKind, DictKey> = {
  setup: "projectActions.kindSetup",
  dev: "projectActions.kindDev",
  test: "projectActions.kindTest",
  cleanup: "projectActions.kindCleanup",
};

type RunStatus = ProjectActionRunStatus | "starting";

const STATUS_KEY: Record<RunStatus, DictKey> = {
  starting: "projectActions.statusStarting",
  confirmation_required: "projectActions.statusConfirmationRequired",
  started: "projectActions.statusStarted",
  running: "projectActions.statusRunning",
  completed: "projectActions.statusCompleted",
  failed: "projectActions.statusFailed",
};

type RunState = {
  status: RunStatus;
  command: string;
  terminalId?: string;
  output: string;
  exitCode?: number;
  signal?: string;
};

type ConfirmPending = {
  actionId: string;
  command: string;
};

/**
 * User-owned setup/dev/test/cleanup actions (P1-14).
 *
 * The dialog is intentionally standalone: it is not mounted by App.tsx or
 * ProjectShelf.tsx yet. It reads the project's `.marionette` action list,
 * shows the exact command before the first run, and streams output through the
 * existing ACP terminal APIs. It never reads package.json or repository
 * scripts, and it never asks the agent layer for a permission mode.
 */
export function ProjectActionsDialog({
  projectId,
  projectName,
  onClose,
}: {
  projectId: string;
  projectName?: string;
  onClose: () => void;
}) {
  const t = useT();
  const [actions, setActions] = useState<ProjectAction[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState("");
  const [savedNote, setSavedNote] = useState("");
  const [run, setRun] = useState<RunState | null>(null);
  const [confirmPending, setConfirmPending] = useState<ConfirmPending | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ProjectAction | null>(null);
  const [waitForSetup, setWaitForSetup] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError("");
    setRun(null);
    setConfirmPending(null);
    setDeleteTarget(null);
    void (async () => {
      try {
        const list = await listProjectActions(projectId);
        if (cancelled) return;
        setActions(list);
        setSelectedId((previous) =>
          previous && list.some((action) => action.id === previous)
            ? previous
            : (list[0]?.id ?? null),
        );
        setDirty(false);
      } catch (loadError) {
        if (!cancelled) setError(String(loadError));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  // The existing background-task APIs already expose every terminal. Poll the
  // one this dialog started; stop the interval as soon as it is no longer
  // running.
  useEffect(() => {
    const terminalId = run?.terminalId;
    if (!terminalId) return;
    if (run?.status !== "started" && run?.status !== "running") return;
    let cancelled = false;
    const tick = async () => {
      const snapshot = await readProjectActionTerminal(terminalId);
      if (cancelled || !snapshot) return;
      setRun((previous) => {
        if (!previous || previous.terminalId !== terminalId) return previous;
        const finished = !snapshot.running;
        const ok = snapshot.exitCode === 0 && !snapshot.signal;
        return {
          ...previous,
          status: finished ? (ok ? "completed" : "failed") : "running",
          output: snapshot.output,
          exitCode: snapshot.exitCode,
          signal: snapshot.signal,
        };
      });
    };
    const timer = window.setInterval(() => {
      void tick();
    }, 800);
    void tick();
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [run?.terminalId, run?.status]);

  const selected = useMemo(
    () => actions.find((action) => action.id === selectedId) ?? null,
    [actions, selectedId],
  );
  const selectedNeedsConfirm = selected ? actionNeedsConfirmation(selected) : false;

  function patchSelected(patch: Partial<ProjectAction>) {
    if (!selectedId) return;
    // The backend fingerprints command + cwd; keep the local badge honest.
    const nextPatch = "cwd" in patch ? { ...patch, confirmedCommand: null } : patch;
    setActions((previous) =>
      previous.map((action) =>
        action.id === selectedId ? { ...action, ...nextPatch } : action,
      ),
    );
    setDirty(true);
    setSavedNote("");
    setConfirmPending(null);
    setRun(null);
  }

  function changeKind(kind: ProjectActionKind) {
    if (!selected) return;
    patchSelected({
      kind,
      runAfterWorktree: kind === "setup" ? selected.runAfterWorktree : false,
    });
  }

  function addAction() {
    const id = `action-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    const next: ProjectAction = {
      id,
      kind: "setup",
      label: t("projectActions.untitled"),
      command: "",
      cwd: "",
      confirmedCommand: null,
      runAfterWorktree: false,
    };
    setActions((previous) => [...previous, next]);
    setSelectedId(id);
    setDirty(true);
    setSavedNote("");
    setConfirmPending(null);
    setRun(null);
  }

  function removeAction(target: ProjectAction) {
    const remaining = actions.filter((action) => action.id !== target.id);
    setActions(remaining);
    setSelectedId(remaining[0]?.id ?? null);
    setDirty(true);
    setSavedNote("");
    setConfirmPending(null);
    setRun(null);
  }

  async function save(): Promise<ProjectAction[] | null> {
    if (actions.some((action) => !action.label.trim())) {
      setError(t("projectActions.nameRequired"));
      return null;
    }
    if (actions.some((action) => !action.command.trim())) {
      setError(t("projectActions.commandRequired"));
      return null;
    }
    setSaving(true);
    setError("");
    try {
      const normalized = await saveProjectActions(projectId, actions);
      setActions(normalized);
      setDirty(false);
      setSavedNote(t("projectActions.saved"));
      setSelectedId((previous) =>
        previous && normalized.some((action) => action.id === previous)
          ? previous
          : (normalized[0]?.id ?? null),
      );
      return normalized;
    } catch (saveError) {
      setError(String(saveError));
      return null;
    } finally {
      setSaving(false);
    }
  }

  async function launch(action: ProjectAction, confirm: boolean) {
    setError("");
    setConfirmPending(null);
    setRun({ status: "starting", command: action.command, output: "" });

    let source = actions;
    if (dirty) {
      const normalized = await save();
      if (!normalized) {
        setRun(null);
        return;
      }
      source = normalized;
    }

    const current = source.find((entry) => entry.id === action.id);
    if (!current) {
      setError(t("projectActions.saveFirst"));
      setRun(null);
      return;
    }
    if (!current.command.trim()) {
      setError(t("projectActions.commandRequired"));
      setRun(null);
      return;
    }

    try {
      const result = await runProjectAction(projectId, current.id, {
        confirm,
        wait: waitForSetup && current.kind === "setup",
      });
      if (result.status === "confirmation_required") {
        setConfirmPending({ actionId: current.id, command: result.command });
        setRun(null);
        return;
      }
      setActions((previous) =>
        previous.map((entry) =>
          entry.id === current.id
            ? { ...entry, confirmedCommand: current.command }
            : entry,
        ),
      );
      setRun({
        status: result.status,
        command: result.command,
        terminalId: result.terminalId,
        output: result.output ?? "",
        exitCode: result.exitCode,
        signal: result.signal,
      });
    } catch (runError) {
      setError(t("projectActions.runFailed", { error: String(runError) }));
      setRun(null);
    }
  }

  function requestRun() {
    if (!selected) return;
    if (!selected.command.trim()) {
      setError(t("projectActions.commandRequired"));
      return;
    }
    if (actionNeedsConfirmation(selected)) {
      setConfirmPending({ actionId: selected.id, command: selected.command.trim() });
      return;
    }
    void launch(selected, false);
  }

  async function stopRun() {
    if (!run?.terminalId) return;
    try {
      await stopProjectAction(run.terminalId);
      setRun((previous) =>
        previous ? { ...previous, status: "failed", signal: "stopped" } : previous,
      );
    } catch (stopError) {
      setError(String(stopError));
    }
  }

  return (
    <DialogShell
      className="project-dialog-backdrop"
      dialogClassName="project-dialog project-actions-dialog"
      labelledBy="project-actions-title"
      onClose={onClose}
    >
      <div className="project-dialog__header">
        <div>
          <strong id="project-actions-title">{t("projectActions.title")}</strong>
          {projectName && <span>{projectName}</span>}
        </div>
        <button
          type="button"
          className="project-dialog__close"
          aria-label={t("common.close")}
          onClick={onClose}
        >
          <X size={16} />
        </button>
      </div>

      <p className="project-actions-dialog__lead">{t("projectActions.lead")}</p>

      <div className="project-actions-dialog__layout">
        <aside className="project-actions-dialog__list">
          <button
            type="button"
            className="project-actions-dialog__add"
            onClick={addAction}
          >
            <Plus size={14} />
            {t("projectActions.add")}
          </button>
          {loading ? (
            <p className="project-actions-dialog__muted">...</p>
          ) : actions.length === 0 ? (
            <p className="project-actions-dialog__muted">{t("projectActions.empty")}</p>
          ) : (
            actions.map((action) => (
              <button
                key={action.id}
                type="button"
                className={`project-actions-dialog__item${action.id === selectedId ? " is-active" : ""}`}
                onClick={() => {
                  setSelectedId(action.id);
                  setConfirmPending(null);
                  setRun(null);
                }}
              >
                <span
                  className={`project-actions-dialog__kind project-actions-dialog__kind--${action.kind}`}
                >
                  {t(KIND_KEY[action.kind])}
                </span>
                <span className="project-actions-dialog__item-label">
                  {action.label || t("projectActions.untitled")}
                </span>
                {actionNeedsConfirmation(action) && (
                  <span className="project-actions-dialog__needs">
                    {t("projectActions.needsConfirm")}
                  </span>
                )}
              </button>
            ))
          )}
        </aside>

        <section className="project-actions-dialog__editor">
          {selected ? (
            <>
              <label className="project-dialog__field">
                <span>{t("projectActions.label")}</span>
                <input
                  value={selected.label}
                  spellCheck={false}
                  placeholder={t("projectActions.labelPlaceholder")}
                  onChange={(event) => patchSelected({ label: event.target.value })}
                />
              </label>

              <label className="project-dialog__field">
                <span>{t("projectActions.kind")}</span>
                <select
                  value={selected.kind}
                  onChange={(event) =>
                    changeKind(event.target.value as ProjectActionKind)
                  }
                >
                  {KIND_OPTIONS.map((option) => (
                    <option key={option.kind} value={option.kind}>
                      {t(option.key)}
                    </option>
                  ))}
                </select>
              </label>

              <label className="project-dialog__field project-actions-dialog__field--stack">
                <span>{t("projectActions.command")}</span>
                <textarea
                  className="project-actions-dialog__command"
                  rows={4}
                  spellCheck={false}
                  autoComplete="off"
                  value={selected.command}
                  placeholder={t("projectActions.commandPlaceholder")}
                  onChange={(event) => patchSelected({ command: event.target.value })}
                />
              </label>

              <label className="project-dialog__field">
                <span>{t("projectActions.cwd")}</span>
                <input
                  value={selected.cwd ?? ""}
                  spellCheck={false}
                  placeholder={t("projectActions.cwdPlaceholder")}
                  onChange={(event) => patchSelected({ cwd: event.target.value })}
                />
              </label>

              {selected.kind === "setup" && (
                <label className="project-actions-dialog__check">
                  <input
                    type="checkbox"
                    checked={selected.runAfterWorktree}
                    onChange={(event) =>
                      patchSelected({ runAfterWorktree: event.target.checked })
                    }
                  />
                  <span>
                    {t("projectActions.runAfterWorktree")}
                    <small>{t("projectActions.runAfterWorktreeHint")}</small>
                  </span>
                </label>
              )}

              <div className="project-actions-dialog__preview">
                <strong>{t("projectActions.command")}</strong>
                <code>{selected.command.trim() || t("projectActions.commandRequired")}</code>
              </div>

              {selectedNeedsConfirm && (
                <p className="project-actions-dialog__needs project-actions-dialog__needs--inline">
                  {t("projectActions.needsConfirm")}
                </p>
              )}

              <label className="project-actions-dialog__check">
                <input
                  type="checkbox"
                  checked={waitForSetup}
                  disabled={selected.kind !== "setup"}
                  onChange={(event) => setWaitForSetup(event.target.checked)}
                />
                <span>
                  {t("projectActions.waitForSetup")}
                  <small>{t("projectActions.waitHint")}</small>
                </span>
              </label>

              <div className="project-actions-dialog__runbar">
                <button
                  type="button"
                  className="project-dialog__submit"
                  disabled={!selected.command.trim() || saving}
                  onClick={requestRun}
                >
                  {t("projectActions.run")}
                </button>
                <button
                  type="button"
                  className="project-dialog__cancel project-actions-dialog__delete"
                  onClick={() => setDeleteTarget(selected)}
                >
                  {t("projectActions.deleteTitle")}
                </button>
              </div>

              {confirmPending && confirmPending.actionId === selected.id && (
                <div className="project-actions-dialog__confirm" role="alert">
                  <strong>{t("projectActions.confirmTitle")}</strong>
                  <p>{t("projectActions.confirmBody")}</p>
                  <code>{confirmPending.command}</code>
                  {selected.cwd && (
                    <p className="project-actions-dialog__muted">
                      {t("projectActions.cwd")}: {selected.cwd}
                    </p>
                  )}
                  <div className="project-actions-dialog__confirm-actions">
                    <button
                      type="button"
                      className="project-dialog__cancel"
                      onClick={() => setConfirmPending(null)}
                    >
                      {t("common.cancel")}
                    </button>
                    <button
                      type="button"
                      className="project-dialog__submit"
                      onClick={() => {
                        const action = actions.find(
                          (entry) => entry.id === confirmPending.actionId,
                        );
                        if (action) void launch(action, true);
                      }}
                    >
                      {t("projectActions.confirmRun")}
                    </button>
                  </div>
                </div>
              )}

              {run && (
                <div className="project-actions-dialog__output">
                  <header>
                    <strong>{t("projectActions.output")}</strong>
                    <span className={`project-actions-dialog__status is-${run.status}`}>
                      {t(STATUS_KEY[run.status])}
                    </span>
                    {run.terminalId &&
                      (run.status === "started" ||
                        run.status === "running" ||
                        run.status === "starting") && (
                        <button
                          type="button"
                          className="project-dialog__cancel"
                          onClick={() => void stopRun()}
                        >
                          <Square size={12} />
                          {t("projectActions.stop")}
                        </button>
                      )}
                  </header>
                  <pre>{run.output || t("projectActions.noOutput")}</pre>
                  {run.exitCode != null && (
                    <p className="project-actions-dialog__muted">
                      {t("projectActions.exitCode", { code: run.exitCode })}
                    </p>
                  )}
                  {run.signal && (
                    <p className="project-actions-dialog__muted">{run.signal}</p>
                  )}
                </div>
              )}

              <p className="project-actions-dialog__security">
                {t("projectActions.securityNote")}
              </p>
            </>
          ) : (
            <p className="project-actions-dialog__muted">{t("projectActions.empty")}</p>
          )}
        </section>
      </div>

      {error && <p className="project-dialog__error">{error}</p>}
      {savedNote && <p className="project-actions-dialog__saved">{savedNote}</p>}

      <div className="project-dialog__actions">
        <button
          type="button"
          className="project-dialog__cancel"
          disabled={saving}
          onClick={onClose}
        >
          {t("common.close")}
        </button>
        <button
          type="button"
          className="project-dialog__submit"
          disabled={!dirty || saving}
          onClick={() => void save()}
        >
          {saving ? "..." : t("common.save")}
        </button>
      </div>

      {deleteTarget && (
        <ConfirmDialog
          title={t("projectActions.deleteTitle")}
          itemName={deleteTarget.label || t("projectActions.untitled")}
          description={t("projectActions.deleteBody")}
          confirmLabel={t("common.delete")}
          cancelLabel={t("common.cancel")}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => {
            const target = deleteTarget;
            setDeleteTarget(null);
            removeAction(target);
          }}
        />
      )}
    </DialogShell>
  );
}
