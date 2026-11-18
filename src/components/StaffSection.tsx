import { useCallback, useEffect, useMemo, useState } from "react";
import { CalendarClock, Check, Edit3, RotateCcw, Save, Trash2, X } from "lucide-react";
import {
  chiefCommitmentAdd,
  chiefCommitmentComplete,
  chiefCommitmentsList,
  chiefStaffMemory,
  chiefStaffMemoryForget,
  chiefStaffMemoryResolveProposal,
  chiefStaffMemoryWrite,
  type ChiefTask,
  type StaffCommitment,
  type StaffMemorySnapshot,
  type StaffProfile,
} from "../lib/api";
import { useT } from "../lib/i18n";
import { ConfirmDialog } from "./ConfirmDialog";

type StaffSectionProps = {
  profile: StaffProfile;
  tasks: ChiefTask[];
  onEdit: (profile: StaffProfile) => void;
  onRemove: (profile: StaffProfile) => Promise<void>;
  onOpenTask?: (task: ChiefTask) => void;
};

const STATUS_LABEL: Record<ChiefTask["status"], string> = {
  proposed: "chief.statusProposed",
  starting: "chief.statusStarting",
  running: "chief.statusRunning",
  waiting_user: "chief.statusWaitingUser",
  completed: "chief.statusCompleted",
  failed: "chief.statusFailed",
  cancelled: "chief.statusCancelled",
  interrupted: "chief.statusInterrupted",
};

function localDateTime(value: number | null | undefined): string {
  if (!value || !Number.isFinite(value)) return "";
  return new Date(value).toLocaleString(undefined, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function StaffSection({
  profile,
  tasks,
  onEdit,
  onRemove,
  onOpenTask,
}: StaffSectionProps) {
  const t = useT();
  const [memory, setMemory] = useState<StaffMemorySnapshot | null>(null);
  const [memoryLayer, setMemoryLayer] = useState<"pinned" | "state">("pinned");
  const [memoryDraft, setMemoryDraft] = useState("");
  const [memoryDirty, setMemoryDirty] = useState(false);
  const [memorySaving, setMemorySaving] = useState(false);
  const [proposalBusy, setProposalBusy] = useState<string | null>(null);
  const [commitments, setCommitments] = useState<StaffCommitment[]>([]);
  const [commitmentText, setCommitmentText] = useState("");
  const [commitmentDue, setCommitmentDue] = useState("");
  const [commitmentBusy, setCommitmentBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [confirmForget, setConfirmForget] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [nextMemory, nextCommitments] = await Promise.all([
        chiefStaffMemory(profile.staffId),
        chiefCommitmentsList(profile.staffId),
      ]);
      setMemory(nextMemory);
      setCommitments(nextCommitments);
      setMemoryDraft(
        memoryLayer === "pinned"
          ? nextMemory?.pinned ?? ""
          : nextMemory?.state ?? "",
      );
      setMemoryDirty(false);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [profile.staffId, memoryLayer]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    setMemoryDraft(memoryLayer === "pinned" ? memory?.pinned ?? "" : memory?.state ?? "");
    setMemoryDirty(false);
  }, [memoryLayer, memory?.pinned, memory?.state, profile.staffId]);

  const profileTasks = useMemo(
    () => tasks
      .filter(
        (task) =>
          task.staffId === profile.staffId ||
          Boolean(profile.sessionId && task.chiefSessionId === profile.sessionId),
      )
      .sort((left, right) => right.updatedAt - left.updatedAt)
      .slice(0, 8),
    [profile.sessionId, tasks],
  );
  const taskTurns = profileTasks.reduce((sum, task) => sum + (task.turnCount ?? 0), 0);
  const taskCost = profileTasks.reduce((sum, task) => sum + (task.costUsd ?? 0), 0);

  const saveMemory = async () => {
    setMemorySaving(true);
    setError(null);
    try {
      const next = await chiefStaffMemoryWrite(profile.staffId, memoryLayer, memoryDraft);
      if (next) setMemory(next);
      setMemoryDirty(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setMemorySaving(false);
    }
  };

  const addCommitment = async () => {
    const text = commitmentText.trim();
    if (!text) return;
    setCommitmentBusy(true);
    setError(null);
    try {
      const due = commitmentDue ? new Date(commitmentDue).getTime() : null;
      await chiefCommitmentAdd(profile.staffId, {
        text,
        kind: due ? "reminder" : "follow_up",
        dueAt: due && Number.isFinite(due) ? due : null,
        condition: null,
        sourceRef: "ui:staff-section",
      });
      setCommitmentText("");
      setCommitmentDue("");
      setCommitments(await chiefCommitmentsList(profile.staffId));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setCommitmentBusy(false);
    }
  };

  const resolveProposal = async (
    proposalId: string,
    accept: boolean,
    layer: "pinned" | "state" = "state",
  ) => {
    setProposalBusy(proposalId);
    setError(null);
    try {
      const next = await chiefStaffMemoryResolveProposal(
        profile.staffId,
        proposalId,
        accept,
        layer,
      );
      if (next) setMemory(next);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setProposalBusy(null);
    }
  };

  const resolveCommitment = async (commitment: StaffCommitment, cancelled: boolean) => {
    setCommitmentBusy(true);
    setError(null);
    try {
      await chiefCommitmentComplete(profile.staffId, commitment.commitmentId, cancelled);
      setCommitments(await chiefCommitmentsList(profile.staffId));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setCommitmentBusy(false);
    }
  };

  const openCommitments = commitments.filter(
    (item) => item.status === "open" || item.status === "due",
  );

  return (
    <div className="staff-section">
      <div className="staff-section__hero">
        <div>
          <strong>{profile.name}</strong>
          <span>{profile.title}</span>
        </div>
        <div className="staff-section__hero-actions">
          <button type="button" className="pill-action" onClick={() => onEdit(profile)}>
            <Edit3 size={12} />
            {t("staff.edit")}
          </button>
          <button
            type="button"
            className="pill-action staff-section__danger"
            onClick={() => setConfirmForget(true)}
          >
            <Trash2 size={12} />
            {t("staff.forgetMemory")}
          </button>
          <button
            type="button"
            className="pill-action staff-section__danger"
            onClick={() => setConfirmRemove(true)}
          >
            <Trash2 size={12} />
            {t("staff.remove")}
          </button>
        </div>
      </div>

      <details className="staff-section__profile-details">
        <summary>{t("staff.profileDetails")}</summary>
        <p className="staff-section__persona">{profile.persona}</p>
        <div className="staff-section__facts">
          <span>{t("staff.agent")}: {profile.agentId}</span>
          <span>{t("staff.autonomy")}: {t(("staff.autonomy" + profile.autonomy.charAt(0).toUpperCase() + profile.autonomy.slice(1)) as never)}</span>
          <span>{t("staff.maxActiveTasks")}: {profile.maxActiveTasks}</span>
          <span>{t("staff.turnUsage", { turns: taskTurns, cost: taskCost.toFixed(4) })}</span>
          <span>{t("staff.projectsCount", { n: profile.allowedProjects.filter((item) => item.enabled).length })}</span>
          {profile.pausedUntilUserResumes ? <span className="is-paused">{t("staff.paused")}</span> : null}
        </div>
      </details>

      <div className="staff-section__subhead">
        <span>{t("staff.tasks")}</span>
        <span className="context-lend__count">{profileTasks.length}</span>
      </div>
      {profileTasks.length === 0 ? (
        <p className="panel-note">{t("staff.tasksEmpty")}</p>
      ) : (
        <div className="staff-section__tasks">
          {profileTasks.map((task) => (
            <div
              className={"chief-task is-" + task.status + (onOpenTask ? " is-clickable" : "")}
              key={task.taskId}
              role={onOpenTask ? "button" : undefined}
              tabIndex={onOpenTask ? 0 : undefined}
              onClick={() => onOpenTask?.(task)}
              onKeyDown={(event) => {
                if (!onOpenTask) return;
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  onOpenTask(task);
                }
              }}
            >
              <div className="chief-task__head">
                <span className="chief-task__title">{task.title}</span>
                <span className="chief-task__status">
                  {t(STATUS_LABEL[task.status] as never)}
                </span>
              </div>
              {task.lastSummary ? <div className="chief-task__summary">{task.lastSummary}</div> : null}
            </div>
          ))}
        </div>
      )}

      <div className="staff-section__subhead">
        <span>{t("staff.memory")}</span>
        <div className="staff-section__tabs">
          <button
            type="button"
            className={memoryLayer === "pinned" ? "is-active" : ""}
            onClick={() => setMemoryLayer("pinned")}
          >
            {t("staff.memoryPinned")}
          </button>
          <button
            type="button"
            className={memoryLayer === "state" ? "is-active" : ""}
            onClick={() => setMemoryLayer("state")}
          >
            {t("staff.memoryState")}
          </button>
        </div>
      </div>
      <textarea
        className="staff-section__memory"
        value={memoryDraft}
        rows={6}
        placeholder={memoryLayer === "pinned" ? t("staff.memoryPinnedHint") : t("staff.memoryStateHint")}
        onChange={(event) => {
          setMemoryDraft(event.target.value);
          setMemoryDirty(true);
        }}
      />
      <div className="staff-section__inline-actions">
        <button type="button" className="pill-action" disabled={!memoryDirty} onClick={() => void load()}>
          <RotateCcw size={12} />
          {t("common.retry")}
        </button>
        <button type="button" className="pill-action is-primary" disabled={!memoryDirty || memorySaving} onClick={() => void saveMemory()}>
          <Save size={12} />
          {memorySaving ? t("staff.saving") : t("staff.saveMemory")}
        </button>
      </div>
      {memory?.recentJournals.length ? (
        <div className="staff-section__journals">
          {memory.recentJournals.slice(0, 3).map((journal) => (
            <details key={journal.date}>
              <summary>{journal.date}</summary>
              <pre>{journal.body}</pre>
            </details>
          ))}
        </div>
      ) : null}
      {memory?.proposals?.length ? (
        <div className="staff-section__proposals">
          <div className="staff-section__subhead">
            <span>{t("staff.memoryProposals")}</span>
            <span className="context-lend__count">{memory.proposals.length}</span>
          </div>
          {memory.proposals.slice(0, 5).map((proposal) => (
            <div className="staff-proposal" key={proposal.proposalId}>
              <div className="staff-proposal__body">
                <strong>{proposal.kind}</strong>
                <span>{proposal.text}</span>
              </div>
              {proposal.status === "pending" ? (
                <div className="staff-proposal__actions">
                  <button
                    type="button"
                    disabled={proposalBusy === proposal.proposalId}
                    title={t("staff.memoryAcceptPinned")}
                    onClick={() => void resolveProposal(proposal.proposalId, true, "pinned")}
                  >
                    <Check size={12} />
                    {t("staff.memoryPinned")}
                  </button>
                  <button
                    type="button"
                    disabled={proposalBusy === proposal.proposalId}
                    title={t("staff.memoryAcceptState")}
                    onClick={() => void resolveProposal(proposal.proposalId, true, "state")}
                  >
                    <Save size={12} />
                    {t("staff.memoryState")}
                  </button>
                  <button
                    type="button"
                    disabled={proposalBusy === proposal.proposalId}
                    title={t("staff.memoryReject")}
                    onClick={() => void resolveProposal(proposal.proposalId, false)}
                  >
                    <X size={12} />
                    {t("staff.memoryReject")}
                  </button>
                </div>
              ) : null}
            </div>
          ))}
        </div>
      ) : null}

      <div className="staff-section__subhead">
        <span>{t("staff.commitments")}</span>
        <span className="context-lend__count">{openCommitments.length}</span>
      </div>
      <div className="staff-section__commitment-form">
        <input
          value={commitmentText}
          placeholder={t("staff.commitmentPlaceholder")}
          onChange={(event) => setCommitmentText(event.target.value)}
        />
        <input
          type="datetime-local"
          value={commitmentDue}
          aria-label={t("staff.commitmentDue")}
          onChange={(event) => setCommitmentDue(event.target.value)}
        />
        <button
          type="button"
          className="pill-action"
          disabled={commitmentBusy || !commitmentText.trim()}
          onClick={() => void addCommitment()}
        >
          <CalendarClock size={12} />
          {t("staff.addCommitment")}
        </button>
      </div>
      {commitments.length === 0 ? (
        <p className="panel-note">{t("staff.commitmentsEmpty")}</p>
      ) : (
        <div className="staff-section__commitments">
          {commitments.slice(0, 10).map((commitment) => {
            const open = commitment.status === "open" || commitment.status === "due";
            return (
              <div key={commitment.commitmentId} className={"staff-commitment is-" + commitment.status}>
                <div>
                  <strong>{commitment.text}</strong>
                  <span>
                    {commitment.status}
                    {commitment.dueAt ? " · " + localDateTime(commitment.dueAt) : ""}
                  </span>
                </div>
                {open ? (
                  <div className="staff-commitment__actions">
                    <button type="button" title={t("staff.commitmentDone")} disabled={commitmentBusy} onClick={() => void resolveCommitment(commitment, false)}>
                      <Check size={12} />
                    </button>
                    <button type="button" title={t("staff.commitmentCancel")} disabled={commitmentBusy} onClick={() => void resolveCommitment(commitment, true)}>
                      <X size={12} />
                    </button>
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      )}

      {error ? <p className="staff-section__error">{error}</p> : null}

      {confirmRemove ? (
        <ConfirmDialog
          title={t("staff.removeTitle")}
          itemName={profile.name}
          description={t("staff.removeDescription")}
          confirmLabel={t("staff.remove")}
          onCancel={() => setConfirmRemove(false)}
          onConfirm={() => {
            setConfirmRemove(false);
            void onRemove(profile);
          }}
        />
      ) : null}
      {confirmForget ? (
        <ConfirmDialog
          title={t("staff.forgetMemoryTitle")}
          itemName={profile.name}
          description={t("staff.forgetMemoryDescription")}
          confirmLabel={t("staff.forgetMemory")}
          onCancel={() => setConfirmForget(false)}
          onConfirm={() => {
            setConfirmForget(false);
            void chiefStaffMemoryForget(profile.staffId)
              .then(load)
              .catch((cause) =>
                setError(cause instanceof Error ? cause.message : String(cause)),
              );
          }}
        />
      ) : null}
    </div>
  );
}
