/**
 * Shared activity-health levels for Clean / Composer / activity bar.
 *
 * Goal: users can tell "still live", "quiet but maybe fine", "likely stuck"
 * without guessing — especially when a tool stays in_progress forever.
 */

export type ActivityHealth = "idle" | "live" | "quiet" | "stalled" | "stuck";

/** Silence thresholds while a turn is active (running / starting). */
export const ACTIVITY_THRESHOLDS = {
  /** First warning: stream has gone quiet. */
  quietMs: 20_000,
  /** Stronger: likely hung mid-tool or mid-think. */
  stalledMs: 60_000,
  /** Almost certainly stuck — recommend interrupt. */
  stuckMs: 120_000,
  /**
   * Nested subagents (OpenCode `task`, etc.) usually emit **zero** parent-ACP
   * events while working. Use a longer fuse so we don't scream "stuck" at 2m
   * when silence is the only wire signal we ever get.
   */
  subagentQuietMs: 90_000,
  subagentStalledMs: 4 * 60_000,
  subagentStuckMs: 8 * 60_000,
} as const;

export type ActivityHealthOpts = {
  /** An in-progress tool is a nested agent (task/subagent) with no parent stream. */
  openSubagent?: boolean;
};

export function activityHealth(
  status: "starting" | "running" | "waiting" | "exited" | "error" | string,
  lastActivityAt: number | null | undefined,
  now: number = Date.now(),
  opts?: ActivityHealthOpts
): ActivityHealth {
  if (status !== "running" && status !== "starting") return "idle";
  if (lastActivityAt == null) return "live";
  const silentFor = now - lastActivityAt;
  if (opts?.openSubagent) {
    // Expected silence while nested agent runs inside the process.
    if (silentFor >= ACTIVITY_THRESHOLDS.subagentStuckMs) return "stuck";
    if (silentFor >= ACTIVITY_THRESHOLDS.subagentStalledMs) return "stalled";
    if (silentFor >= ACTIVITY_THRESHOLDS.subagentQuietMs) return "quiet";
    return "live";
  }
  if (silentFor >= ACTIVITY_THRESHOLDS.stuckMs) return "stuck";
  if (silentFor >= ACTIVITY_THRESHOLDS.stalledMs) return "stalled";
  if (silentFor >= ACTIVITY_THRESHOLDS.quietMs) return "quiet";
  return "live";
}

export function isToolInProgress(status: string | undefined | null): boolean {
  if (!status) return false;
  // Terminal statuses: completed / failed / cancelled / error / success / rejected …
  // ACP may also send "updated" mid-flight — treat open tools as in-progress only
  // for clearly non-terminal verbs.
  if (/complet|fail|cancel|error|success|reject|abort|denied|timeout/i.test(status)) {
    return false;
  }
  return /run|progress|in_progress|pending|updated/i.test(status);
}

export function formatAgo(ms: number, now: number = Date.now()): string {
  const sec = Math.max(0, Math.floor((now - ms) / 1000));
  if (sec < 2) return "just now";
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  return `${hr}h ${min % 60}m ago`;
}

export function activityBarLabel(
  status: "starting" | "running" | "waiting" | "exited" | "error" | string,
  health: ActivityHealth,
  opts?: ActivityHealthOpts
): string {
  if (status === "starting") {
    if (health === "stuck" || health === "stalled") return "Connecting failed to progress…";
    if (health === "quiet") return "Connecting is slow…";
    return "Connecting…";
  }
  if (status === "running") {
    // OpenCode/Claude nested agents: parent ACP is often silent on purpose.
    if (opts?.openSubagent) {
      if (health === "stuck") return "Subagent silent too long — likely waiting on hidden approval";
      if (health === "stalled") return "Subagent still silent — may be stuck on permissions";
      if (health === "quiet") return "Subagent running (no nested stream over ACP)";
      return "Subagent running (no nested stream over ACP)";
    }
    if (health === "stuck") return "Appears stuck — no stream updates";
    if (health === "stalled") return "Likely stalled — no updates recently";
    if (health === "quiet") return "Still working… no updates recently";
    return "Working";
  }
  if (status === "error") return "Error";
  return "";
}

/**
 * Tools that run a nested agent inside the agent process.
 *
 * These are the ones that go silent: a subagent's approval prompt is handled
 * inside its own runtime, and at least opencode's `task` never forwards it over
 * ACP — so the client is never asked, nothing appears on screen, and the tool
 * sits `in_progress` forever. Observed: two 6-minute hangs with 2–4 ACP events
 * total and zero `session/request_permission`, while the same read done by the
 * main model asked for permission in 1ms and finished in 19s.
 *
 * `title` matters: OpenCode often renames the card to the agent/prompt while
 * the stable tool name is still `task` (or arrives late).
 */
export function isSubagentTool(
  toolName: string | null | undefined,
  title?: string | null,
): boolean {
  const name = (toolName ?? "").trim();
  if (
    /^(task|agent|subagent|delegate|dispatch_agent|spawn_agent|call_omo_agent)$/i.test(name)
  ) {
    return true;
  }
  // Normalized aliases / partial names (not todowrite).
  if (name && /task|subagent|spawn_agent|dispatch_agent/i.test(name) && !/todo/i.test(name)) {
    return true;
  }
  const t = (title ?? "").trim();
  if (!t) return false;
  if (/\b(subagent|sub-agent|nested agent|task agent|delegate agent)\b/i.test(t)) return true;
  // OpenCode task cards often look like: "Task (general)" / "general · explore …"
  if (/^task\b/i.test(t)) return true;
  if (/\(general\)|\(explore\)|\(scout\)/i.test(t)) return true;
  return false;
}

export function stallBannerCopy(health: ActivityHealth, opts: {
  midTurn: boolean;
  openToolTitle?: string | null;
  /** The open tool runs a nested agent — different failure mode, different advice. */
  openToolIsSubagent?: boolean;
  ago?: string;
}): { title: string; body: string } | null {
  // Expected subagent silence: no red banner while still "live".
  if (health === "idle" || health === "live") return null;

  const ago = opts.ago ? ` (last update ${opts.ago})` : "";
  const toolBit = opts.openToolTitle
    ? ` Last open tool: ${opts.openToolTitle}.`
    : "";

  // Quiet under a nested agent is normal — explain once, don't alarm.
  if (opts.openToolIsSubagent && health === "quiet") {
    return {
      title: "Subagent running silently",
      body:
        `OpenCode/Claude nested agents do not stream their inner turns over ACP${ago}.${toolBit} ` +
        `You will only see this tool flip to completed when the child finishes. ` +
        `If it never returns, it may be waiting on a permission dialog inside the agent process ` +
        `(not forwarded to Marionette) — interrupt with Esc×2 / ■ and run the step here instead.`,
    };
  }

  // A silent subagent is almost never "thinking" — name the real cause.
  if (opts.openToolIsSubagent && (health === "stalled" || health === "stuck")) {
    return {
      title: "Subagent has gone silent too long",
      body:
        `No parent-ACP updates${ago}.${toolBit} Nested agents run inside the agent process; their ` +
        `permission prompts are not forwarded over ACP — if it touched a path needing approval, ` +
        `it is waiting on a dialog that can never reach you. ` +
        `Interrupt with Esc×2 or ■, then either do the step in this dialog, use Marionette @-delegate ` +
        `(full child stream), or pre-approve paths in the agent's own config.`,
    };
  }

  if (health === "stuck") {
    return {
      title: "Agent appears stuck",
      body: opts.midTurn
        ? `No stream updates for 2+ minutes${ago}.${toolBit} The turn may be hung inside a tool or model call. Interrupt to regain control, then send again.`
        : `No stream updates for 2+ minutes${ago}. Interrupt if you need to send a new message.`,
    };
  }
  if (health === "stalled") {
    return {
      title: "No updates for a while",
      body: opts.midTurn
        ? `Stream has been silent for 1+ minute${ago}.${toolBit} It may still be working, or it may be stuck. Interrupt anytime with Esc×2 or ■.`
        : `Still waiting for the first chunk${ago}. Interrupt if this hangs.`,
    };
  }
  // quiet
  return {
    title: opts.midTurn ? "Stream quiet" : "Still waiting",
    body: opts.midTurn
      ? `No new thinking / tool / reply updates recently${ago}.${toolBit} Long tools can be silent — if it feels frozen, interrupt.`
      : `Message sent. Waiting for thinking / tools / reply${ago}.`,
  };
}

export function composerBusyCopy(
  health: ActivityHealth,
  opts?: ActivityHealthOpts
): string {
  if (opts?.openSubagent) {
    if (health === "stuck") {
      return "Subagent silent too long — Esc×2 or ■ (may be hidden permission)";
    }
    if (health === "stalled") {
      return "Subagent still silent — Esc×2 or ■ if it feels hung";
    }
    return "Subagent running (no nested ACP stream) — Esc×2 or ■ to interrupt";
  }
  if (health === "stuck") {
    return "Appears stuck — Esc×2 or ■ to interrupt and regain control";
  }
  if (health === "stalled") {
    return "No updates for 1m+ — may be stuck. Esc×2 or ■ to interrupt";
  }
  if (health === "quiet") {
    return "Still working (no recent stream updates) — Esc×2 or ■ to interrupt";
  }
  return "Agent working — Esc×2 or ■ to interrupt";
}
