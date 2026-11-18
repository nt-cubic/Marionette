import { Pause, Play, Plus, UserRound } from "lucide-react";
import { useT } from "../lib/i18n";
import type { StaffDisplayState } from "../lib/staffStatus";

export type StaffRosterState = StaffDisplayState;

export interface StaffRosterItem {
  staffId: string;
  sessionId?: string | null;
  name: string;
  title: string;
  displayState: StaffRosterState;
  primaryTaskTitle?: string | null;
  taskCount?: number;
  dueCommitmentCount?: number;
  waitingReason?: string | null;
  paused?: boolean;
}

type StaffRosterProps = {
  staff: StaffRosterItem[];
  activeSessionId?: string | null;
  paused?: boolean;
  onSelect: (staff: StaffRosterItem) => void;
  onHire: () => void;
  onTogglePause?: () => void;
};

const STATE_LABEL: Record<StaffRosterState, string> = {
  waiting_user: "staff.stateWaiting",
  failed: "staff.stateFailed",
  interrupted: "staff.stateInterrupted",
  running: "staff.stateRunning",
  thinking: "staff.stateThinking",
  idle: "staff.stateIdle",
};

/**
 * Persistent people-first roster above projects/chat.
 *
 * This is a projection: state comes from tasks/commitments and never becomes
 * its own persisted truth. Long-lived staff rows stay even when worker cards
 * are gone.
 */
export function StaffRoster({
  staff,
  activeSessionId,
  paused = false,
  onSelect,
  onHire,
  onTogglePause,
}: StaffRosterProps) {
  const t = useT();
  const hasPausableWork = staff.some(
    (member) =>
      member.paused ||
      member.displayState !== "idle" ||
      (member.taskCount ?? 0) > 0 ||
      (member.dueCommitmentCount ?? 0) > 0,
  );
  return (
    <div className="staff-roster" aria-label={t("staff.rosterAria")}>
      <div className="shelf-group staff-roster__head">
        <span className="shelf-group__label">{t("staff.group")}</span>
        <div className="staff-roster__actions">
          {onTogglePause && (paused || hasPausableWork) ? (
            <button
              className={"staff-roster__pause" + (paused ? " is-paused" : "")}
              type="button"
              title={paused ? t("staff.resumeAll") : t("staff.pauseAll")}
              aria-label={paused ? t("staff.resumeAll") : t("staff.pauseAll")}
              onClick={onTogglePause}
            >
              {paused ? <Play size={11} /> : <Pause size={11} />}
            </button>
          ) : null}
          <button
            className="shelf-group__add"
            type="button"
            title={t("staff.hire")}
            aria-label={t("staff.hire")}
            onClick={onHire}
          >
            <Plus size={12} />
          </button>
        </div>
      </div>

      {staff.length === 0 ? (
        <button className="staff-roster__empty" type="button" onClick={onHire}>
          <UserRound size={13} />
          <span>{t("staff.hireFirst")}</span>
        </button>
      ) : (
        staff.map((member) => {
          const active = Boolean(member.sessionId && member.sessionId === activeSessionId);
          const stateLabel = t(STATE_LABEL[member.displayState] as never);
          const taskCount = Math.max(0, member.taskCount ?? 0);
          const dueCommitmentCount = Math.max(0, member.dueCommitmentCount ?? 0);
          const title = member.primaryTaskTitle || member.name + " · " + member.title;
          const className =
            "staff-row is-" +
            member.displayState +
            (active ? " is-active" : "") +
            (member.paused ? " is-paused" : "");
          return (
            <button
              key={member.staffId}
              className={className}
              type="button"
              title={title}
              onClick={() => onSelect(member)}
            >
              <span className="staff-row__avatar" aria-hidden>
                <UserRound size={13} />
              </span>
              <span className="staff-row__content">
                <span className="staff-row__name">
                  <strong>{member.name}</strong>
                  <em>{member.title}</em>
                </span>
                <span className="staff-row__meta">
                  <span className="staff-row__state">{stateLabel}</span>
                  {member.primaryTaskTitle ? (
                    <span className="staff-row__task">{member.primaryTaskTitle}</span>
                  ) : null}
                  {taskCount > 1 ? <span className="staff-row__count">{taskCount}</span> : null}
                  {dueCommitmentCount > 0 ? (
                    <span
                      className="staff-row__due"
                      title={t("staff.commitmentsDueShort", { n: dueCommitmentCount })}
                    >
                      {dueCommitmentCount}
                    </span>
                  ) : null}
                </span>
              </span>
            </button>
          );
        })
      )}
    </div>
  );
}
