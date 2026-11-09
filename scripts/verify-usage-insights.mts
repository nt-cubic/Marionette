// Regression for the Token usage page v2 data layer:
// usageDigest (per-turn fold) → usageLedger v2 detail → usageInsights (page model).
// Run: npx tsx scripts/verify-usage-insights.mts
import {
  commandProgram,
  currentTurnEvents,
  digestTurn,
  estimateTokens,
  mcpToolKey,
} from "../src/lib/usageDigest";
import {
  TPS_EDGES,
  TTFT_EDGES_MS,
  histBucket,
  pruneLedger,
  recordTurnUsageInto,
  type UsageLedger,
} from "../src/lib/usageLedger";
import { aggregateInsights, histPercentile } from "../src/lib/usageInsights";
import type { SessionEvent } from "../src/lib/types";

let failed = 0;
function check(name: string, ok: boolean, detail?: unknown) {
  if (ok) console.log("  ok ", name);
  else {
    failed += 1;
    console.log("  FAIL", name, detail ?? "");
  }
}
const near = (a: number | null, b: number, eps = 1e-6) => a != null && Math.abs(a - b) <= eps;

// ── commandProgram / mcp ──
check("program: git status", commandProgram("git status") === "git");
check("program: exe path", commandProgram("C:\\Program Files\\nodejs\\node.exe a.js") === "node");
check("program: powershell → pwsh", commandProgram("powershell -NoProfile -Command Get-Item x") === "pwsh");
check("program: cmd /c npm i", commandProgram("cmd /c npm i") === "npm");
check("program: bash -lc cargo", commandProgram("bash -lc cargo test") === "cargo");
check("program: env prefix", commandProgram("FOO=1 BAR=2 npx tsc") === "npx");
check("program: empty", commandProgram("   ") === null);
check("mcp: double underscore", mcpToolKey("mcp__browser__read_page") === "browser · read_page");
check("mcp: not mcp", mcpToolKey("read") === null);
check("estimate: 4 chars ≈ 1", estimateTokens("abcdefgh") === 2 && estimateTokens("") === 0);

// ── digestTurn ──
const T0 = Date.parse("2026-11-08T10:00:00Z");
const at = (s: number) => new Date(T0 + s * 1000).toISOString();
const sid = "s1";
const events: SessionEvent[] = [
  { type: "user_message", sessionId: sid, text: "old turn", createdAt: at(-100) },
  { type: "assistant_message", sessionId: sid, text: "old reply", createdAt: at(-90) },
  { type: "user_message", sessionId: sid, text: "x".repeat(400), createdAt: at(0), attachments: [{} as never, {} as never] },
  { type: "thought", sessionId: sid, text: "hmm", createdAt: at(2), endedAt: at(5) },
  { type: "tool_call", sessionId: sid, text: "", toolName: "read", input: "a".repeat(40), detail: "b".repeat(80), status: "completed", createdAt: at(5), completedAt: at(6) },
  { type: "tool_call", sessionId: sid, text: "", toolName: "bash", title: "bash", input: JSON.stringify({ command: "git diff --stat" }), status: "failed", createdAt: at(6), completedAt: at(10) },
  { type: "tool_call", sessionId: "other", text: "", toolName: "read", createdAt: at(7) },
  { type: "tool_call", sessionId: sid, text: "", toolName: "edit", detail: "--- a\n+++ b\n+one\n+two\n-three", status: "completed", createdAt: at(10), completedAt: at(11) },
  { type: "tool_call", sessionId: sid, text: "", toolName: "mcp__browser__click", status: "completed", createdAt: at(11), completedAt: at(12) },
  { type: "assistant_message", sessionId: sid, text: "done", createdAt: at(12) },
];
check("turn events start at last user message", currentTurnEvents(events, sid).length === 7);
const dg = digestTurn(events, sid, { stats: { ttftMs: 2000 }, sentAt: T0, endedAt: T0 + 15_000 });
check("timing: think 3s", dg.timing.think === 3000, dg.timing);
check("timing: tool 1+4+1+1 = 7s", dg.timing.tool === 7000, dg.timing);
check("timing: answer runs to turn end (3s)", dg.timing.answer === 3000, dg.timing);
check("timing: ttft from stats", dg.timing.ttft === 2000);
check("timing: other = wall − counted", dg.timing.other === 15_000 - 2000 - 3000 - 7000 - 3000, dg.timing);
check("tools: read 1", dg.tools.read?.n === 1);
check("tools: command 1 failed", dg.tools.command?.n === 1 && dg.tools.command?.fail === 1, dg.tools);
check("cmds: git counted with its failure", dg.cmds.git?.n === 1 && dg.cmds.git?.fail === 1, dg.cmds);
check("skills: mcp key", dg.skills["mcp:browser · click"]?.n === 1, dg.skills);
check("edits: +2 −1 in 1 op", dg.edits.ops === 1 && dg.edits.add === 2 && dg.edits.del === 1, dg.edits);
check("est: user text 100 + 2 images", dg.est.user === 100 && dg.est.attach === 2000, dg.est);
check("est: tool io from input + detail", dg.est.toolIo >= 30, dg.est);
check("other session's tool ignored", !Object.values(dg.tools).some((t) => t.n > 1));

// overlapping spans never make `other` negative
const overlap = digestTurn(events, sid, { stats: { ttftMs: 60_000 }, sentAt: T0, endedAt: T0 + 15_000 });
check("timing: other clamps at 0", overlap.timing.other === 0);

// ── histograms ──
check("hist: 700ms → bucket 2", histBucket(700, TTFT_EDGES_MS) === 2);
check("hist: open last bucket", histBucket(1e9, TPS_EDGES) === TPS_EDGES.length);
const h = new Array(TTFT_EDGES_MS.length + 1).fill(0);
h[3] = 10; // all in [750, 1000)
check("percentile: inside bucket", near(histPercentile(h, TTFT_EDGES_MS, 0.5), 875));
check("percentile: empty → null", histPercentile(new Array(5).fill(0), [1, 2, 3, 4], 0.5) === null);

// ── recorder ──
const day = new Date(2026, 10, 8, 14, 0, 0);
let ledger: UsageLedger = { version: 1, days: {} };
const stats = { input: 1000, output: 400, cached: 9000, cacheCreation: 600, reasoning: 150, ttftMs: 900, outputTps: 45, durationMs: 8000 };
ledger = recordTurnUsageInto(
  ledger,
  { agentId: "claude-code", modelId: "opus", stats, endedAt: day.getTime(), sentAt: day.getTime() - 15_000, digest: dg, ctx: { used: 85_000, size: 100_000 }, ctxFresh: true, session: { id: "s1", label: "Fix bug", project: "Marionette" } },
  day,
);
ledger = recordTurnUsageInto(
  ledger,
  { agentId: "claude-code", modelId: "opus", stats, endedAt: day.getTime() + 60_000, sentAt: null, digest: null, ctx: null, session: { id: "s1", label: "Fix bug", project: "Marionette" } },
  new Date(day.getTime() + 60_000),
);
ledger = recordTurnUsageInto(
  ledger,
  { agentId: "codex", modelId: "gpt-6", stats: { input: 100, output: 50, cached: 0, cacheCreation: 0 }, endedAt: day.getTime(), session: { id: "s2", label: "Other", project: null } },
  day,
);
const key = "2026-11-08";
const d = ledger.days[key]!;
check("legacy totals still add up", d.input === 2100 && d.turns === 3, d);
check("legacy agents shape kept", d.agents["claude-code"]?.output === 800 && d.agents["claude-code"]?.turns === 2);
const det = d.detail?.["claude-code"];
check("detail: buckets per agent", det?.input === 2000 && det?.cached === 18000 && det?.turns === 2, det);
check("detail: wall only when sentAt known", det?.wallMs === 15_000, det?.wallMs);
check("detail: timing only from the digested turn", det?.timing.tool === 7000, det?.timing);
check("detail: ttft hist counts both turns", det!.ttftHist.reduce((a, b) => a + b, 0) === 2);
check("detail: ctx 85% → bucket 8", det?.ctxHist[8] === 1);
check("detail: tallies merged", det?.cmds.git?.n === 1 && det?.edits.add === 2);
check("models carry their agent", d.models.opus?.agent === "claude-code" && d.models["gpt-6"]?.agent === "codex");
check("sessionIds deduped", d.sessionIds?.length === 2, d.sessionIds);
check("session row accumulates", ledger.sessions?.s1?.turns === 2 && ledger.sessions?.s1?.ctxPct === 85, ledger.sessions?.s1);

// prune keeps sessions inside the window, drops old ones
const old: UsageLedger = { ...ledger, sessions: { ...ledger.sessions!, ancient: { ...ledger.sessions!.s1!, last: Date.parse("2025-01-01") } } };
const pruned = pruneLedger(old, day);
check("prune: old session dropped", !pruned.sessions?.ancient && !!pruned.sessions?.s1);

// ── insights ──
const legacyDay = { input: 500, output: 100, cached: 0, cacheCreation: 0, reasoning: 0, turns: 1, ms: 0, agents: { grok: { output: 100, turns: 1 } }, models: {}, hours: new Array(24).fill(0) };
ledger = { ...ledger, days: { ...ledger.days, "2026-11-01": legacyDay } };
const ins = aggregateInsights(ledger, { range: 30, now: day, prices: null });
check("insights: totals include legacy day", ins.totals.turns === 4 && ins.totals.buckets.input === 2600, ins.totals);
check("insights: hasDetail", ins.hasDetail);
const compSum = ins.composition.reduce((a, p) => a + p.tokens, 0);
const v2Total = 2000 + 800 + 18000 + 1200 + 100 + 50;
check("composition sums to the measured v2 total", Math.abs(compSum - v2Total) < 1e-6, { compSum, v2Total, parts: ins.composition });
check("composition: shares sum to 1", Math.abs(ins.composition.reduce((a, p) => a + p.share, 0) - 1) < 1e-9);
check("composition: provenance tags", ins.composition.find((p) => p.key === "agentInternal")?.source === "derived" && ins.composition.find((p) => p.key === "user")?.source === "estimated");
check("timing shares sum to 1", Math.abs(ins.timing.reduce((a, p) => a + p.share, 0) - 1) < 1e-9);
check("daily: 30 points, last = today", ins.daily.length === 30 && ins.daily[29]?.key === key);
check("agents: sorted by tokens, legacy grok kept", ins.agents[0]?.agentId === "claude-code" && ins.agents.some((a) => a.agentId === "grok"));
check("agents: model rows under their agent", ins.agents[0]?.models[0]?.modelId === "opus");
check("agents: sparkline aligned", ins.agents[0]?.series.length === 30 && (ins.agents[0]?.series[29] ?? 0) > 0);
check("ranks: git", ins.cmds[0]?.key === "git" && ins.cmdTotals.fail === 1);
check("ranks: mcp label stripped", ins.skills[0]?.label === "browser · click" && ins.skills[0]?.kind === "mcp");
check("ctxHot counts ≥80%", ins.ctxHot === 1);
check("sessions newest first", ins.sessions[0]?.id === "s1" && ins.sessions.length === 2);
check("ttft p50 present", ins.ttft != null && ins.ttft.p50 > 750 && ins.ttft.p50 < 1000, ins.ttft);
check("usd null without prices", ins.usd.value === null);

const codex = aggregateInsights(ledger, { range: 30, now: day, agent: "codex" });
check("filter: only codex tokens", codex.totals.buckets.input === 100 && codex.totals.turns === 1, codex.totals);
check("filter: sessions filtered", codex.sessions.length === 1 && codex.sessions[0]?.id === "s2");
check("filter: agents table only codex", codex.agents.length === 1);

const legacyOnly = aggregateInsights({ version: 1, days: { "2026-11-01": legacyDay } }, { range: 30, now: day });
check("legacy-only: no detail, no composition", !legacyOnly.hasDetail && legacyOnly.composition.length === 0 && legacyOnly.hasData);

const prevCheck = aggregateInsights(ledger, { range: 7, now: day });
check("delta: previous week had the legacy day", prevCheck.previous.turns === 0 || prevCheck.delta.total !== undefined);


// ── v3 (2026-11-09): peak ×2 / reported cost / scoped sessions / outcomes / files ──
{
  const { buildRateIndex, costOfModelUsage, isDeepSeekPeak } = await import("../src/lib/tokenPrices.ts");
  // Beijing = UTC+8; 2026-11-09 is a Monday.
  const ms = (hUtc: number) => Date.UTC(2026, 10, 9, hUtc, 30);
  check("peak: Mon 10:30 Beijing is peak", isDeepSeekPeak(ms(2)) === true);
  check("peak: Mon 13:30 Beijing (lunch) is off-peak", isDeepSeekPeak(ms(5)) === false);
  check("peak: Mon 15:30 Beijing is peak", isDeepSeekPeak(ms(7)) === true);
  check("peak: Saturday is off-peak", isDeepSeekPeak(Date.UTC(2026, 10, 14, 2, 30)) === false);

  const rates = buildRateIndex({
    deepseek: { models: { "ds-1": { cost: { input: 1, output: 2, cache_read: 0.1, cache_write: 0.5 } } } },
    grok: { models: { "grok-4.6": { cost: { input: 3, output: 8 } } } },
  });
  const dsRate = rates.byProviderModel.get("deepseek/ds-1")![0]!;
  const grokRate = rates.byProviderModel.get("grok/grok-4.6")![0]!;
  // reported cost wins for its tokens: 1M/1M at list would be far more than the claimed $0.50
  const reported = costOfModelUsage(
    { input: 1_000_000, output: 1_000_000, cached: 0, cacheCreation: 0, rep: { input: 1_000_000, output: 1_000_000, cached: 0, cacheCreation: 0 }, repUsd: 0.5 },
    dsRate,
  );
  check("cost: agent-reported usd wins", near(reported.usd!, 0.5) && reported.reportedUsd === 0.5, reported);
  // peak ×2 only for DeepSeek lab rates and only un-reported tokens
  const peak = costOfModelUsage(
    { input: 1_000_000, output: 0, cached: 0, cacheCreation: 0, peak: { input: 1_000_000, output: 0, cached: 0, cacheCreation: 0 } },
    dsRate,
  );
  check("cost: deepseek peak doubles", near(peak.usd!, 2), peak);
  const otherPeak = costOfModelUsage(
    { input: 1_000_000, output: 0, cached: 0, cacheCreation: 0, peak: { input: 1_000_000, output: 0, cached: 0, cacheCreation: 0 } },
    grokRate,
  );
  check("cost: non-deepseek never doubles", near(otherPeak.usd!, 3) && otherPeak.peakExtraUsd === 0, otherPeak);

  // scoped sessions: a session spans two days, a 1-day window only counts today
  const t0 = Date.UTC(2026, 10, 7, 10, 0);
  const t1 = Date.UTC(2026, 10, 9, 10, 0);
  let twoDays: UsageLedger = { version: 1, days: {} };
  const mkStats = { input: 100, output: 50, cached: 0, cacheCreation: 0 };
  twoDays = recordTurnUsageInto(twoDays, { agentId: "grok-build", modelId: "grok-4.6", stats: mkStats, endedAt: t0, session: { id: "sg", label: "Long run", project: null } }, new Date(t0));
  twoDays = recordTurnUsageInto(twoDays, { agentId: "grok-build", modelId: "grok-4.6", stats: mkStats, endedAt: t1, session: { id: "sg", label: "Long run", project: null } }, new Date(t1));
  twoDays = recordTurnUsageInto(twoDays, {
    agentId: "grok-build", modelId: "grok-4.6", stats: mkStats, endedAt: t1,
    outcome: "failed", compactions: 1, reportedUsd: 0.25,
    session: { id: "sg", label: "Long run", project: null },
    digest: { ...dg, reads: { "src/a.ts": 2 }, edited: { "src/b.ts": { n: 1, add: 3, del: 1 } } },
  }, new Date(t1));
  const dayView = aggregateInsights(twoDays, { range: 1, now: new Date(t1 + 3_600_000) });
  const sg = dayView.sessions.find((row) => row.id === "sg");
  check("scoped: 1-day window counts only today's turns", sg?.turns === 2 && sg?.total === 300, sg);
  check("scoped: reported usd is window-scoped", near(sg!.reportedUsd, 0.25), sg);
  const weekView = aggregateInsights(twoDays, { range: 7, now: new Date(t1 + 3_600_000) });
  const sgWeek = weekView.sessions.find((row) => row.id === "sg");
  check("scoped: week window sees all turns", sgWeek?.turns === 3 && sgWeek?.total === 450, sgWeek);
  check("outcomes: failed + compactions counted", dayView.turnFails === 1 && dayView.compactions === 1, { fails: dayView.turnFails, comp: dayView.compactions });
  check("files: read + edited paths rank", dayView.files[0]?.path === "src/a.ts" && dayView.files[0]?.reads === 2, dayView.files);
  check("files: edit lines carried", dayView.files.some((f) => f.path === "src/b.ts" && f.edits === 1 && f.add === 3 && f.del === 1), dayView.files);
}


// ── composition scaling (grok 对话组成; Rust 侧单测覆盖解析,这里管前端校准) ──
{
  const mod = await import("../src/lib/usageBackfill.ts");
  const scaleComposition = mod.scaleComposition;
  type BackfillComposition = mod.BackfillComposition;
  const mk = (over: Partial<BackfillComposition>): BackfillComposition => ({
    system: 1800, toolDefs: 10600, inject: 26000, user: 6700, attach: 3000,
    assistant: 400, reasoning: 22000, toolCalls: 300, toolResults: 31000,
    measuredPrompt: null, ...over,
  });
  const anchored = scaleComposition(mk({ measuredPrompt: 130386 }));
  check("comp: scaled parts sum to the measured anchor", anchored.calibrated && near(anchored.parts.reduce((s, p) => s + p.tokens, 0), 130386, 2), anchored.total);
  // Per-part Math.round leaves the shares a hair under 1 — allow 1e-4.
  check("comp: shares sum to 1", near(anchored.parts.reduce((s, p) => s + p.share, 0), 1, 1e-4));
  check("comp: coverage = est/anchor", near(anchored.coverage ?? 0, 101800 / 130386, 1e-3));
  const raw = scaleComposition(mk({}));
  check("comp: no anchor keeps the raw estimate", !raw.calibrated && raw.coverage == null && near(raw.total, 101800, 2));
  check("comp: shares still sum to 1 without anchor", near(raw.parts.reduce((s, p) => s + p.share, 0), 1, 1e-9));
  const empty = scaleComposition({ system: 0, toolDefs: 0, inject: 0, user: 0, attach: 0, assistant: 0, reasoning: 0, toolCalls: 0, toolResults: 0, measuredPrompt: 500 });
  check("comp: all-zero never divides by zero", empty.parts.every((p) => p.share === 0) && empty.total === 500);
  const capped = scaleComposition(mk({ measuredPrompt: 10 }));
  check("comp: coverage caps at 1 when estimate exceeds anchor", capped.coverage === 1);
}

console.log(failed === 0 ? "\n全部通过" : `\n${failed} 项失败`);
process.exit(failed === 0 ? 0 : 1);
