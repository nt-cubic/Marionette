import {
  extractUsageFromAcpData,
  parseCodexStatusRateLimits,
  mergeUsageFromAcp,
  buildUsageSnapshot,
} from "../src/lib/usage.ts";

const u1 = extractUsageFromAcpData({
  params: {
    update: {
      sessionUpdate: "usage_update",
      used: 53000,
      size: 200000,
      cost: { amount: 0.045, currency: "USD" },
    },
  },
});
console.log("context", u1);

const claude = extractUsageFromAcpData({
  params: {
    update: {
      sessionUpdate: "usage_update",
      used: 1000,
      size: 200000,
      _meta: {
        "_claude/rateLimit": {
          status: "allowed",
          rateLimitType: "five_hour",
          utilization: 0.42,
          resetsAt: Math.floor(Date.now() / 1000) + 7200,
        },
      },
    },
  },
});
console.log("claude rate", claude?.rateWindows);

const status = parseCodexStatusRateLimits(
  "**Token usage:** 1k\n**5h limit:** 58% left · resets in 2h\n**Weekly limit:** 90% left"
);
console.log("codex status", status);

let state = mergeUsageFromAcp(undefined, {
  params: { update: { sessionUpdate: "usage_update", used: 10, size: 100 } },
});
state = mergeUsageFromAcp(state!, {
  params: {
    update: {
      sessionUpdate: "usage_update",
      used: 20,
      size: 100,
      _meta: {
        "_claude/rateLimit": { rateLimitType: "seven_day", utilization: 12 },
      },
    },
  },
});
const snap = buildUsageSnapshot({
  agentId: "claude-code",
  agentLabel: "Claude",
  state: state!,
  connected: true,
});
console.log(JSON.stringify(snap, null, 2));
