/**
 * Usage-panel smoke test.
 *
 * Run: npx tsx scripts/usage-smoke.mts   (or `node --experimental-strip-types`)
 *
 * Every payload below is a real capture from `initialize`/`session/prompt`
 * against the installed adapters, so these assertions pin the wire shapes we
 * actually have to parse — not an idealised reading of the ACP spec.
 */
import assert from "node:assert/strict";
import {
  emptySessionUsage,
  buildUsageSnapshot,
  extractTurnTokens,
  extractUsageFromAcpData,
  mergeGrokBilling,
  mergeUsageFromAcp,
  mergeUsageFromPromptResult,
  mergeUsageFromText,
  parseClaudeUsageText,
  parseCodexStatusRateLimits,
  parseGrokBilling,
  parseGrokCostText,
  seedContextSize,
} from "../src/lib/usage.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

console.log("usage_update (standard ACP)");

check("context + cost parse out of a usage_update", () => {
  const u = extractUsageFromAcpData({
    params: {
      update: {
        sessionUpdate: "usage_update",
        used: 53000,
        size: 200000,
        cost: { amount: 0.045, currency: "USD" },
      },
    },
  });
  assert.equal(u?.contextUsed, 53000);
  assert.equal(u?.contextSize, 200000);
  assert.equal(u?.costAmount, 0.045);
  assert.equal(u?.costCurrency, "USD");
});

check("Claude _claude/rateLimit meta becomes a rate_limit window", () => {
  const u = extractUsageFromAcpData({
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
  const w = u?.rateWindows.find((r) => r.id === "five-hour");
  assert.ok(w, "expected a five-hour window");
  assert.equal(w.percentage, 42);
  assert.equal(w.kind, "rate_limit");
});

check("Codex /status markdown yields used% (not the printed left%)", () => {
  const rows = parseCodexStatusRateLimits(
    "**Token usage:** 1k\n**5h limit:** 58% left · resets in 2h\n**Weekly limit:** 90% left"
  );
  const five = rows.find((r) => r.id === "five-hour");
  assert.equal(five?.percentage, 42, "58% left must render as 42% used");
  assert.ok(rows.some((r) => r.id === "weekly"));
});

console.log("\nClaude /usage (the only on-demand source for plan limits)");

// Verbatim capture from claude-agent-acp 0.61.0.
const CLAUDE_USAGE = `You are currently using your subscription to power your Claude Code usage

Current session: 18% used · resets Jul 28, 3:10am (Asia/Tokyo)
Current week (all models): 68% used · resets Jul 30, 7pm (Asia/Tokyo)

What's contributing to your limits usage?
Last 24h · 47 requests · 1 session
  100% of your usage was at >150k context`;

check("session + weekly parse out of real /usage output", () => {
  const rows = parseClaudeUsageText(CLAUDE_USAGE);
  const five = rows.find((r) => r.id === "five-hour");
  const weekly = rows.find((r) => r.id === "weekly");
  assert.equal(five?.percentage, 18, "these are % used, not % left");
  assert.equal(five?.label, "5-hour limit");
  assert.match(five?.detail ?? "", /resets Jul 28/);
  assert.equal(weekly?.percentage, 68);
  assert.equal(weekly?.label, "Weekly limit");
});

check("prose lines with % are not mistaken for limits", () => {
  // "100% of your usage was at >150k context" must not become a window.
  const rows = parseClaudeUsageText(CLAUDE_USAGE);
  assert.equal(rows.length, 2, `got ${JSON.stringify(rows.map((r) => r.id))}`);
});

check("per-model weekly buckets get their own row", () => {
  const rows = parseClaudeUsageText(
    "Current week (Opus): 4% used · resets Jul 30, 7pm\nCurrent week (all models): 68% used"
  );
  assert.equal(rows.find((r) => r.id === "weekly-opus")?.percentage, 4);
  assert.equal(rows.find((r) => r.id === "weekly-opus")?.label, "Weekly (Opus)");
  assert.equal(rows.find((r) => r.id === "weekly")?.percentage, 68);
});

check("/usage text flows through mergeUsageFromText into the panel", () => {
  const state = mergeUsageFromText(undefined, CLAUDE_USAGE);
  assert.ok(state, "expected /usage text to produce usage state");
  const snap = buildUsageSnapshot({
    agentId: "claude-code",
    agentLabel: "Claude Code",
    state: state!,
    connected: true,
  });
  assert.equal(snap.windows.find((w) => w.id === "five-hour")?.percentage, 18);
  assert.equal(snap.windows.find((w) => w.id === "weekly")?.percentage, 68);
  // The "click refresh" hint must be gone once the rows are real.
  assert.doesNotMatch(snap.note ?? "", /Plan limits/);
});

check("Codex /status still parses as % left", () => {
  // Guard the two formats against each other: Codex prints remaining, Claude used.
  const rows = parseCodexStatusRateLimits("**5h limit:** 58% left");
  assert.equal(rows[0]?.percentage, 42);
  assert.deepEqual(parseClaudeUsageText("**5h limit:** 58% left"), []);
});

console.log("\nsession/prompt result (the only usage Grok reports)");

check("Grok: _meta token counts, no usage_update anywhere", () => {
  // Real Grok 0.2.104 turn result.
  const tokens = extractTurnTokens({
    result: {
      stopReason: "end_turn",
      _meta: {
        totalTokens: 12759,
        modelId: "grok-4.5",
        inputTokens: 12729,
        outputTokens: 29,
        cachedReadTokens: 2816,
      },
    },
  });
  assert.deepEqual(tokens, {
    input: 12729,
    output: 29,
    cached: 2816,
    reasoning: null,
    total: 12759,
  });
});

check("Claude / OpenCode: standard PromptResponse.usage", () => {
  const tokens = extractTurnTokens({
    result: {
      stopReason: "end_turn",
      usage: {
        inputTokens: 2,
        outputTokens: 4,
        cachedReadTokens: 0,
        cachedWriteTokens: 38061,
        totalTokens: 38067,
      },
    },
  });
  assert.equal(tokens?.input, 2);
  assert.equal(tokens?.total, 38067);
});

check("Codex: _meta.quota.token_count naming (cachedInputTokens)", () => {
  const tokens = extractTurnTokens({
    result: {
      stopReason: "end_turn",
      usage: { totalTokens: 14201, inputTokens: 4212, outputTokens: 5 },
      _meta: {
        quota: {
          token_count: {
            totalTokens: 14201,
            inputTokens: 4212,
            cachedInputTokens: 9984,
            outputTokens: 5,
            reasoningOutputTokens: 0,
          },
        },
      },
    },
  });
  assert.equal(tokens?.cached, 9984);
  assert.equal(tokens?.total, 14201);
});

check("a session/update notification is not mistaken for a turn result", () => {
  assert.equal(
    extractTurnTokens({
      method: "session/update",
      params: { update: { sessionUpdate: "usage_update", used: 1, size: 2 } },
    }),
    null
  );
});

check("Grok _x.ai/session_notification turn_completed.usage", () => {
  // Live capture from grok agent stdio 0.2.104
  const tokens = extractTurnTokens({
    method: "_x.ai/session_notification",
    params: {
      sessionId: "s1",
      update: {
        sessionUpdate: "turn_completed",
        usage: {
          inputTokens: 12826,
          outputTokens: 37,
          totalTokens: 12863,
          cachedReadTokens: 2816,
          reasoningTokens: 32,
        },
      },
    },
  });
  assert.equal(tokens?.input, 12826);
  assert.equal(tokens?.output, 37);
  assert.equal(tokens?.cached, 2816);
  assert.equal(tokens?.reasoning, 32);
  assert.equal(tokens?.total, 12863);
});

check("usage_update stays authoritative for contextUsed", () => {
  const withUpdate = mergeUsageFromAcp(undefined, {
    params: { update: { sessionUpdate: "usage_update", used: 500, size: 200000 } },
  });
  const after = mergeUsageFromPromptResult(withUpdate!, {
    result: { usage: { inputTokens: 9, outputTokens: 1, totalTokens: 10 } },
  });
  assert.equal(after?.contextUsed, 500, "turn total must not clobber usage_update");
  assert.equal(after?.turnTokens?.total, 10);
});

check("with no usage_update, the turn total seeds contextUsed", () => {
  const grok = mergeUsageFromPromptResult(undefined, {
    result: { _meta: { totalTokens: 12759, inputTokens: 12729, outputTokens: 29 } },
  });
  assert.equal(grok?.contextUsed, 12759);
});

console.log("\nGrok account weekly credits (_x.ai/billing / TUI /usage)");

check("parses live _x.ai/billing shape", () => {
  const parsed = parseGrokBilling({
    config: {
      creditUsagePercent: 5,
      currentPeriod: {
        type: "USAGE_PERIOD_TYPE_WEEKLY",
        start: "2026-07-24T16:12:39.595605+00:00",
        end: "2026-07-31T16:12:39.595605+00:00",
      },
      billingPeriodEnd: "2026-07-31T16:12:39.595605+00:00",
    },
    subscription_tier: "SuperGrok",
  });
  assert.ok(parsed);
  assert.equal(parsed!.tier, "SuperGrok");
  assert.equal(parsed!.windows.length, 1);
  assert.equal(parsed!.windows[0].id, "weekly");
  assert.equal(parsed!.windows[0].percentage, 5);
  assert.ok(parsed!.windows[0].detail?.toLowerCase().includes("reset"));
});

check("parses TUI Weekly limit text fallback", () => {
  const rows = parseGrokCostText(
    "Weekly limit: 5%\nNext reset: August 1, 01:12"
  );
  assert.equal(rows[0]?.id, "weekly");
  assert.equal(rows[0]?.percentage, 5);
  assert.ok(rows[0]?.detail?.includes("August 1"));
});

check("mergeGrokBilling paints a weekly row for grok-build snapshot", () => {
  const state = mergeGrokBilling(undefined, {
    config: {
      creditUsagePercent: 5,
      currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY", end: "2026-07-31T16:12:39Z" },
    },
    subscription_tier: "SuperGrok",
  });
  const snap = buildUsageSnapshot({
    agentId: "grok-build",
    agentLabel: "Grok Build",
    state: state!,
    connected: true,
  });
  const weekly = snap.windows.find((w) => w.id === "weekly");
  assert.ok(weekly, "expected weekly row");
  assert.equal(weekly!.percentage, 5);
});

console.log("\nsession/ready context ceiling");

check("seeds a size, then refuses to overwrite a live one", () => {
  const seeded = seedContextSize(undefined, 500000);
  assert.equal(seeded?.contextSize, 500000);
  assert.equal(seedContextSize(seeded!, 123), null, "must not overwrite");
  assert.equal(seedContextSize(undefined, 0), null, "0 is not a ceiling");
  assert.equal(seedContextSize(undefined, null), null);
});

console.log("\nsnapshot rendering");

check("Grok shows a real meter from set-up size + turn result", () => {
  let state = seedContextSize(undefined, 500000)!;
  state = mergeUsageFromPromptResult(state, {
    result: { _meta: { totalTokens: 12759, inputTokens: 12729, outputTokens: 29, cachedReadTokens: 2816 } },
  })!;
  const snap = buildUsageSnapshot({
    agentId: "grok-build",
    agentLabel: "Grok Build",
    state,
    connected: true,
  });
  const ctx = snap.windows.find((w) => w.id === "context");
  assert.equal(ctx?.percentage, 2.6, "12759/500000 = 2.6%");
  const turn = snap.windows.find((w) => w.id === "turn");
  // formatTokenCount rounds to whole K at/above 10K, keeps a decimal below it.
  assert.equal(turn?.detail, "13K in · 29 out · 2.8K cached");
  // The old dead branch keyed on "grok" and could never fire for this agent.
  assert.ok(
    !snap.windows.some((w) => w.detail === "Waiting for agent usage_update"),
    "no stale placeholder row"
  );
});

check("Claude with no rate_limit_event shows no empty limit rows", () => {
  const state = mergeUsageFromAcp(undefined, {
    params: {
      update: {
        sessionUpdate: "usage_update",
        used: 38067,
        size: 1000000,
        cost: { amount: 0.229013, currency: "USD" },
      },
    },
  })!;
  const snap = buildUsageSnapshot({
    agentId: "claude-code",
    agentLabel: "Claude Code",
    state,
    connected: true,
  });
  assert.ok(
    !snap.windows.some((w) => w.kind === "rate_limit"),
    "limit rows must be omitted until the agent reports them"
  );
  assert.ok(
    snap.windows.some((w) => w.id === "cost" && w.detail?.includes("0.229")),
    "cost row missing"
  );
  assert.match(snap.note ?? "", /Plan limits/, "absence must be explained in the note");
});

check("a warmed-but-unused session says what to do, not 'waiting'", () => {
  // Exactly the reported state: Claude connected, agent warmed, zero turns sent,
  // user hit refresh (which stamps refreshedAt + source but carries no data).
  const state = {
    ...emptySessionUsage(),
    refreshedAt: new Date().toISOString(),
    source: "manual refresh",
  };
  const snap = buildUsageSnapshot({
    agentId: "claude-code",
    agentLabel: "Claude Code",
    state,
    connected: true,
  });
  const ctx = snap.windows.find((w) => w.id === "context");
  assert.equal(ctx?.detail, "Send a message to fill this in");
  // A bare "Source: manual refresh" told the user nothing about why it is empty.
  assert.doesNotMatch(snap.note ?? "", /Source: manual refresh/);
  assert.match(snap.note ?? "", /after the first turn/);
});

check("real data still wins over the hint", () => {
  const state = mergeUsageFromAcp(undefined, {
    params: { update: { sessionUpdate: "usage_update", used: 124299, size: 1000000 } },
  })!;
  const snap = buildUsageSnapshot({
    agentId: "claude-code",
    agentLabel: "Claude Code",
    state: { ...state, source: "manual refresh" },
    connected: true,
  });
  assert.equal(snap.windows.find((w) => w.id === "context")?.percentage, 12.4);
  assert.match(snap.note ?? "", /Source: manual refresh/);
});

check("a reported limit still renders", () => {
  const state = mergeUsageFromAcp(undefined, {
    params: {
      update: {
        sessionUpdate: "usage_update",
        used: 20,
        size: 100,
        _meta: { "_claude/rateLimit": { rateLimitType: "seven_day", utilization: 12 } },
      },
    },
  })!;
  const snap = buildUsageSnapshot({
    agentId: "claude-code",
    agentLabel: "Claude Code",
    state,
    connected: true,
  });
  const weekly = snap.windows.find((w) => w.id === "weekly");
  assert.equal(weekly?.percentage, 12);
});

console.log(`\n${passed} checks passed.`);
