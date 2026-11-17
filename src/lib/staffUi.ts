import type {
  ChiefProjectPolicy,
  ChiefTask,
  StaffCommitment,
  StaffProfile,
} from "./api";
import type {
  StaffHireDraft,
  StaffProjectDraftPolicy,
} from "../components/StaffHireDialog";
import type { StaffRosterItem, StaffRosterState } from "../components/StaffRoster";
import { projectStaffStatus } from "./staffStatus";

export function staffForSession(
  staff: readonly StaffProfile[],
  sessionId: string | null | undefined,
): StaffProfile | null {
  if (!sessionId) return null;
  return staff.find((member) => member.sessionId === sessionId) ?? null;
}

export function buildStaffRoster(
  staff: readonly StaffProfile[],
  tasks: readonly ChiefTask[],
  globalPaused: boolean,
  commitmentsByStaff: ReadonlyMap<string, readonly StaffCommitment[]> = new Map(),
): StaffRosterItem[] {
  return staff.map((member) => {
    const memberTasks = member.sessionId
      ? tasks.filter(
          (task) =>
            task.staffId === member.staffId || task.chiefSessionId === member.sessionId,
        )
      : tasks.filter((task) => task.staffId === member.staffId);
    const projected = projectStaffStatus(memberTasks, {
      commitments: commitmentsByStaff.get(member.staffId) ?? [],
    });
    const primaryTask = projected.primaryTaskId
      ? memberTasks.find((task) => task.taskId === projected.primaryTaskId)
      : null;
    return {
      staffId: member.staffId,
      sessionId: member.sessionId,
      name: member.name,
      title: member.title,
      displayState: projected.displayState as StaffRosterState,
      primaryTaskTitle: primaryTask?.title ?? null,
      taskCount: projected.counts.active,
      dueCommitmentCount: projected.dueCommitmentCount,
      waitingReason: projected.waitingReason,
      paused: globalPaused || member.pausedUntilUserResumes,
    };
  });
}

function projectPolicyForDraft(
  projectId: string,
  existing: ChiefProjectPolicy | undefined,
  requested: StaffProjectDraftPolicy | undefined,
): ChiefProjectPolicy {
  return {
    projectId,
    enabled: true,
    remoteMode: requested?.remoteMode ?? existing?.remoteMode ?? "read_only",
    allowedAgents: existing?.allowedAgents ?? [],
    defaultAgentId: existing?.defaultAgentId ?? null,
    defaultModel: existing?.defaultModel ?? null,
    maxConcurrentWorkers: existing?.maxConcurrentWorkers ?? 1,
    maxTaskMinutes: existing?.maxTaskMinutes ?? 120,
    allowComputerUse: requested?.allowComputerUse ?? existing?.allowComputerUse ?? false,
    selfModification: requested?.selfModification ?? existing?.selfModification ?? false,
  };
}

function telegramChatIdFromDraft(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  const parsed = Number(trimmed);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

export function staffProfileFromDraft(
  draft: StaffHireDraft,
  existing: StaffProfile | null,
  now = Date.now(),
): StaffProfile {
  const currentPolicies = new Map(
    (existing?.allowedProjects ?? []).map((policy) => [policy.projectId, policy]),
  );
  return {
    staffId: existing?.staffId ?? draft.staffId ?? "",
    name: draft.name.trim(),
    title: draft.title.trim(),
    persona: draft.persona.trim(),
    agentId: draft.agentId,
    model: draft.model.trim() || null,
    allowedProjects: draft.allowedProjectIds.map((projectId) =>
      projectPolicyForDraft(
        projectId,
        currentPolicies.get(projectId),
        draft.projectPolicies[projectId],
      ),
    ),
    sessionId: existing?.sessionId ?? "",
    telegramChatId: telegramChatIdFromDraft(draft.telegramChatId),
    autonomy: draft.autonomy,
    maxActiveTasks: Math.max(1, Math.min(3, draft.maxActiveTasks || 1)),
    maxTurnsPerTask: draft.maxTurnsPerTask,
    maxWallMinutesPerTask: draft.maxWallMinutesPerTask,
    maxModelCallsPerTask: draft.maxModelCallsPerTask,
    maxCostPerTaskUsd: draft.maxCostPerTaskUsd,
    maxReportsPerHour: draft.maxReportsPerHour,
    notificationCooldownMinutes: draft.notificationCooldownMinutes,
    quietHours: draft.quietHours.trim(),
    allowBackgroundWork: draft.allowBackgroundWork,
    allowTelegram: draft.allowTelegram,
    pausedUntilUserResumes: existing?.pausedUntilUserResumes ?? false,
    createdAt: existing?.createdAt ?? now,
    updatedAt: now,
  };
}

export function staffDraftFromProfile(profile: StaffProfile): StaffHireDraft {
  return {
    staffId: profile.staffId,
    name: profile.name,
    title: profile.title,
    persona: profile.persona,
    agentId: profile.agentId,
    model: profile.model ?? "",
    telegramChatId: profile.telegramChatId ? String(profile.telegramChatId) : "",
    autonomy: profile.autonomy,
    maxActiveTasks: profile.maxActiveTasks,
    maxTurnsPerTask: profile.maxTurnsPerTask,
    maxWallMinutesPerTask: profile.maxWallMinutesPerTask,
    maxModelCallsPerTask: profile.maxModelCallsPerTask,
    maxCostPerTaskUsd: profile.maxCostPerTaskUsd,
    maxReportsPerHour: profile.maxReportsPerHour,
    notificationCooldownMinutes: profile.notificationCooldownMinutes,
    quietHours: profile.quietHours ?? "",
    allowBackgroundWork: profile.allowBackgroundWork,
    allowTelegram: profile.allowTelegram,
    allowedProjectIds: profile.allowedProjects
      .filter((policy) => policy.enabled)
      .map((policy) => policy.projectId),
    projectPolicies: Object.fromEntries(
      profile.allowedProjects
        .filter((policy) => policy.enabled)
        .map((policy) => [
          policy.projectId,
          {
            remoteMode:
              policy.remoteMode === "write_preauthorized"
                ? "write_preauthorized"
                : policy.remoteMode === "approve_writes"
                  ? "approve_writes"
                  : "read_only",
            allowComputerUse: policy.allowComputerUse,
            selfModification: policy.selfModification,
          } satisfies StaffProjectDraftPolicy,
        ]),
    ),
  };
}
