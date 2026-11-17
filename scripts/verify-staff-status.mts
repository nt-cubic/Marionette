/**
 * verify-staff-status — pure employee status projection and notification gates.
 *
 * Run: npx tsx scripts/verify-staff-status.mts
 */
import assert from "node:assert/strict";
import {
  DEFAULT_STAFF_STALE_AFTER_MS,
  evaluateStaffNotificationGate,
  isNotificationCooldownActive,
  isNotificationHourlyCapReached,
  isNotificationPersistentlyPaused,
  isStaffTaskStale,
  isWithinQuietHours,
  projectStaffStatus,
  shouldBypassNotificationGate,
  shouldRetainTerminalTask,
} from "../src/lib/staffStatus.ts";
import type {
  StaffCommitmentInput,
  StaffNotificationKind,
  StaffTaskLike,
} from "../src/lib/staffStatus.ts";
import {
  buildStaffRoster,
  staffDraftFromProfile,
  staffProfileFromDraft,
} from "../src/lib/staffUi.ts";

let passed = 0;

function check(name: string, fn: () => void): void {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

function task(
  taskId: string,
  status: StaffTaskLike["status"],
  eventAt: number,
  extra: Partial<StaffTaskLike> = {},
): StaffTaskLike {
  return {
    taskId,
    status,
    createdAt: eventAt,
    updatedAt: eventAt,
    lastEventAt: eventAt,
    ...extra,
  };
}

const now = Date.UTC(2026, 10, 17, 12, 0, 0);
const minute = 60_000;

console.log("staff status projection");

check("waiting_user outranks failed, running, starting and proposed", () => {
  const projected = projectStaffStatus(
    [
      task("proposed", "proposed", now - 5 * minute),
      task("starting", "starting", now - 4 * minute),
      task("running", "running", now - 3 * minute),
      task("failed", "failed", now - 2 * minute),
      task("waiting", "waiting_user", now - minute, {
        waitingReason: "permission",
      }),
    ],
    { now },
  );
  assert.equal(projected.displayState, "waiting_user");
  assert.equal(projected.primaryTaskId, "waiting");
  assert.equal(projected.waitingReason, "permission");
});

check("failed and interrupted stay distinct and outrank running", () => {
  const failed = projectStaffStatus(
    [
      task("running", "running", now - minute),
      task("failed", "failed", now - 2 * minute),
    ],
    { now },
  );
  const interrupted = projectStaffStatus(
    [
      task("running", "running", now - minute),
      task("interrupted", "interrupted", now - 2 * minute),
    ],
    { now },
  );
  assert.equal(failed.displayState, "failed");
  assert.equal(failed.primaryTaskId, "failed");
  assert.equal(interrupted.displayState, "interrupted");
  assert.equal(interrupted.primaryTaskId, "interrupted");
});

check("same failed/interrupted tier picks the most recent task", () => {
  const projected = projectStaffStatus(
    [
      task("failed", "failed", now - 3 * minute),
      task("interrupted", "interrupted", now - minute),
    ],
    { now },
  );
  assert.equal(projected.displayState, "interrupted");
  assert.equal(projected.primaryTaskId, "interrupted");
});

check("missing waiting reason becomes explicit unknown", () => {
  const projected = projectStaffStatus(
    [task("waiting", "waiting_user", now - minute)],
    { now },
  );
  assert.equal(projected.waitingReason, "unknown");
});

check("counts cover every status plus active, terminal and stale", () => {
  const projected = projectStaffStatus(
    [
      task("proposed", "proposed", now - minute),
      task("starting", "starting", now - minute),
      task("running", "running", now - DEFAULT_STAFF_STALE_AFTER_MS * 2),
      task("waiting", "waiting_user", now - minute),
      task("completed", "completed", now - minute),
      task("failed", "failed", now - minute),
      task("interrupted", "interrupted", now - minute),
      task("cancelled", "cancelled", now - minute),
    ],
    { now },
  );
  assert.deepEqual(projected.counts, {
    total: 8,
    proposed: 1,
    starting: 1,
    running: 1,
    waitingUser: 1,
    completed: 1,
    failed: 1,
    interrupted: 1,
    cancelled: 1,
    active: 4,
    terminal: 4,
    stale: 1,
  });
  assert.deepEqual(projected.staleTaskIds, ["running"]);
});

check("stale is configurable and does not rewrite running into failure", () => {
  const running = task("running", "running", now - 10 * minute);
  assert.equal(isStaffTaskStale(running, now, 5 * minute), true);
  assert.equal(isStaffTaskStale(running, now, 20 * minute), false);
  const projected = projectStaffStatus([running], { now, staleAfterMs: 5 * minute });
  assert.equal(projected.displayState, "running");
  assert.deepEqual(projected.staleTaskIds, ["running"]);
});

check("terminal retention is configurable and only gates display selection", () => {
  const failed = task("failed", "failed", now - 10 * minute);
  assert.equal(shouldRetainTerminalTask(failed, now, 5 * minute), false);
  const expired = projectStaffStatus([failed], { now, terminalRetentionMs: 5 * minute });
  assert.equal(expired.displayState, "idle");
  assert.equal(expired.counts.failed, 1);
  const retained = projectStaffStatus([failed], { now, terminalRetentionMs: 20 * minute });
  assert.equal(retained.displayState, "failed");
});

check("last timestamps derive from available facts; report waits for report input", () => {
  const projected = projectStaffStatus(
    [
      task("old", "completed", now - 10 * minute, {
        updatedAt: now - 8 * minute,
        lastProgressAt: now - 7 * minute,
        lastReportAt: now - 6 * minute,
      }),
      task("new", "running", now - 2 * minute),
    ],
    { now, notification: { eventKind: "progress" } },
  );
  assert.equal(projected.lastEventAt, now - 2 * minute);
  assert.equal(projected.lastProgressAt, now - 2 * minute);
  assert.equal(projected.lastReportAt, now - 6 * minute);
});

check("due commitments are counted without becoming a display state", () => {
  const commitments: StaffCommitmentInput[] = [
    { commitmentId: "future", status: "open", dueAt: now + minute },
    { commitmentId: "due-time", status: "open", dueAt: now - minute },
    { commitmentId: "due-state", status: "due" },
    { commitmentId: "done", status: "done", dueAt: now - minute },
  ];
  const projected = projectStaffStatus([], { now, commitments });
  assert.equal(projected.displayState, "idle");
  assert.deepEqual(projected.dueCommitmentIds, ["due-time", "due-state"]);
  assert.equal(projected.dueCommitmentCount, 2);
});

console.log("notification gates");

check("priority set matches the Rust notification gate", () => {
  const expected: Record<StaffNotificationKind, boolean> = {
    waiting_user: true,
    failed: true,
    commitment_due: true,
    interrupted: true,
    permission: true,
    question: true,
    plan: true,
    completed: false,
    progress: false,
  };
  for (const [kind, bypass] of Object.entries(expected)) {
    assert.equal(shouldBypassNotificationGate(kind as StaffNotificationKind), bypass);
  }
});

check("persistent pause blocks even priority notifications", () => {
  assert.equal(isNotificationPersistentlyPaused(true), true);
  assert.equal(isNotificationPersistentlyPaused(false), false);
  assert.equal(isNotificationPersistentlyPaused(undefined), false);
  const decision = evaluateStaffNotificationGate({
    now,
    eventKind: "waiting_user",
    pausedUntilUserResumes: true,
  });
  assert.deepEqual(decision, {
    allowed: false,
    bypassed: false,
    reason: "persistent_pause",
  });
});

check("quiet hours support overnight ranges and deterministic timezone offsets", () => {
  const atTokyo2300 = Date.UTC(2026, 10, 17, 14, 0, 0);
  const atTokyo0700 = Date.UTC(2026, 10, 17, 22, 0, 0);
  const atTokyo0800 = Date.UTC(2026, 10, 17, 23, 0, 0);
  assert.equal(isWithinQuietHours(atTokyo2300, "23:00-08:00", 540), true);
  assert.equal(isWithinQuietHours(atTokyo0700, "23:00-08:00", 540), true);
  assert.equal(isWithinQuietHours(atTokyo0800, "23:00-08:00", 540), false);
});

check("ordinary notifications respect quiet hours", () => {
  const decision = evaluateStaffNotificationGate({
    now: Date.UTC(2026, 10, 17, 14, 0, 0),
    eventKind: "progress",
    quietHours: "23:00-08:00",
    timezoneOffsetMinutes: 540,
  });
  assert.equal(decision.allowed, false);
  assert.equal(decision.reason, "quiet_hours");
});

check("cooldown blocks ordinary notifications up to, not at, its boundary", () => {
  assert.equal(isNotificationCooldownActive(now, now - 29 * minute, 30 * minute), true);
  assert.equal(isNotificationCooldownActive(now, now - 30 * minute, 30 * minute), false);
  const decision = evaluateStaffNotificationGate({
    now,
    eventKind: "completed",
    lastNotificationAt: now - 10 * minute,
    cooldownMs: 30 * minute,
  });
  assert.equal(decision.allowed, false);
  assert.equal(decision.reason, "cooldown");
});

check("hourly cap blocks ordinary notifications and 0 means unlimited", () => {
  assert.equal(isNotificationHourlyCapReached(3, 4), false);
  assert.equal(isNotificationHourlyCapReached(4, 4), true);
  assert.equal(isNotificationHourlyCapReached(99, 0), false);
  const decision = evaluateStaffNotificationGate({
    now,
    eventKind: "progress",
    notificationsLastHour: 4,
    maxReportsPerHour: 4,
  });
  assert.equal(decision.allowed, false);
  assert.equal(decision.reason, "hourly_cap");
});

check("priority events bypass quiet hours, cooldown and hourly cap", () => {
  for (const eventKind of [
    "waiting_user",
    "failed",
    "interrupted",
    "commitment_due",
    "permission",
    "question",
    "plan",
  ] as const) {
    const decision = evaluateStaffNotificationGate({
      now: Date.UTC(2026, 10, 17, 14, 0, 0),
      eventKind,
      quietHours: "23:00-08:00",
      timezoneOffsetMinutes: 540,
      lastNotificationAt: now - minute,
      cooldownMs: 30 * minute,
      notificationsLastHour: 99,
      maxReportsPerHour: 4,
    });
    assert.deepEqual(decision, {
      allowed: true,
      bypassed: true,
      reason: "priority_bypass",
    });
  }
});

check("staff project permissions round-trip without widening the default", () => {
  const profile = staffProfileFromDraft(
    {
      name: "Reviewer",
      title: "Read-only reviewer",
      persona: "Review code.",
      agentId: "codex",
      model: "",
      telegramChatId: "",
      autonomy: "assisted",
      maxActiveTasks: 1,
      maxTurnsPerTask: 30,
      maxWallMinutesPerTask: 120,
      maxModelCallsPerTask: 0,
      maxCostPerTaskUsd: 0,
      maxReportsPerHour: 4,
      notificationCooldownMinutes: 30,
      quietHours: "23:00-08:00",
      allowBackgroundWork: true,
      allowTelegram: false,
      allowedProjectIds: ["project_safe"],
      projectPolicies: {
        project_safe: {
          remoteMode: "read_only",
          allowComputerUse: false,
          selfModification: false,
        },
      },
    },
    null,
    now,
  );
  assert.equal(profile.allowedProjects[0]?.remoteMode, "read_only");
  assert.equal(profile.allowedProjects[0]?.allowComputerUse, false);
  const draft = staffDraftFromProfile(profile);
  assert.equal(draft.projectPolicies.project_safe?.remoteMode, "read_only");
});

check("roster carries due commitments without changing the display state", () => {
  const profile = staffProfileFromDraft(
    {
      name: "Follow-up owner",
      title: "Assistant",
      persona: "Track follow-ups.",
      agentId: "codex",
      model: "",
      telegramChatId: "",
      autonomy: "assisted",
      maxActiveTasks: 1,
      maxTurnsPerTask: 30,
      maxWallMinutesPerTask: 120,
      maxModelCallsPerTask: 0,
      maxCostPerTaskUsd: 0,
      maxReportsPerHour: 4,
      notificationCooldownMinutes: 30,
      quietHours: "",
      allowBackgroundWork: true,
      allowTelegram: false,
      allowedProjectIds: [],
      projectPolicies: {},
    },
    null,
    now,
  );
  const roster = buildStaffRoster(
    [profile],
    [],
    false,
    new Map([
      [
        profile.staffId,
        [
          {
            commitmentId: "cmt_due",
            staffId: profile.staffId,
            text: "Follow up",
            kind: "follow_up",
            dueAt: now - 1,
            status: "due",
            createdAt: now - minute,
          },
        ],
      ],
    ]),
  );
  assert.equal(roster[0]?.displayState, "idle");
  assert.equal(roster[0]?.dueCommitmentCount, 1);
});

console.log(`\nverify-staff-status: ${passed} checks passed`);
