import type { SessionStatus } from "./types";

export type UsageProbeOutcome = "deferred" | "sent";

/**
 * Run the hidden local usage command for an ACP-backed subscription.
 *
 * Restored dialogs are persisted as `exited`; a live process can also disappear
 * between turns. Clicking refresh must therefore warm/validate ACP first instead
 * of only sending while an already-live status happens to say waiting. A real
 * turn in flight owns the one-prompt wire slot, so defer to the post-turn probe.
 */
export async function runUsageProbe(args: {
  status: SessionStatus;
  ensureReady: () => Promise<unknown>;
  send: () => Promise<unknown>;
}): Promise<UsageProbeOutcome> {
  if (args.status === "running") return "deferred";
  await args.ensureReady();
  await args.send();
  return "sent";
}
