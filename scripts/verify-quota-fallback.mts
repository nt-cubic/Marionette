/**
 * Own-family engine quota fallback (2026-11-11).
 *
 * 背景：原生探针（codex /status、grok _x.ai/billing）在回合进行中必被
 * 「interrupt it first」拒绝、挂起会话没有进程可问——恰是用户去看额度的
 * 时刻。own-family 引擎行从「整族跳过」改为「按语义槽位兜底」：原生探针
 * 盖住的槽位原生赢，缺位引擎顶上，点一下即显（网关 kept=1 缓存毫秒级）。
 *
 * Run: npx tsx scripts/verify-quota-fallback.mts
 */
import assert from "node:assert/strict";

import type { EngineQuotaEntry } from "../src/lib/api";
import { buildUsageSnapshot, emptySessionUsage } from "../src/lib/usage";
import { buildQuotaView, isExhausted, meterTone, pctLevel, quotaSlotKey } from "../src/lib/quotaRail";
import type { UsageWindow } from "../src/lib/types";

let passed = 0;
function ok(name: string, cond: boolean) {
  assert.ok(cond, name);
  passed += 1;
  console.log(`  ✓ ${name}`);
}

// ── quotaSlotKey：原生 id 与引擎窗口名归一到同一语义槽位 ──────────────────
console.log("quotaSlotKey normalization");
ok("native five-hour → 5h", quotaSlotKey("five-hour") === "5h");
ok("engine 5 hours → 5h", quotaSlotKey("5 hours") === "5h");
ok("native 5h → 5h", quotaSlotKey("5h") === "5h");
ok("native weekly = engine 7 days", quotaSlotKey("weekly") === quotaSlotKey("7 days"));
ok("engine 7 days → weekly", quotaSlotKey("7 days") === "weekly");
ok("native monthly stays monthly", quotaSlotKey("monthly") === "monthly");
ok("per-model weekly-opus is its own slot", quotaSlotKey("weekly-opus") !== quotaSlotKey("weekly"));
ok("MCP · Month is the mcp slot", quotaSlotKey("MCP · Month") === "mcp");

// ── 夹具：网关实测形状（kept=1 读数） ─────────────────────────────────────
const codexEntry: EngineQuotaEntry = {
  provider: "codex",
  name: "Codex",
  user: "a@example.com",
  windows: [
    { name: "5 hours", used: 10, remaining: 90, resetsAt: "2026-11-11T21:04:23+09:00" },
    { name: "7 days", used: 91, remaining: 9, resetsAt: "2026-11-11T22:52:30+09:00" },
  ],
};
const grokEntry: EngineQuotaEntry = {
  provider: "grok",
  name: "Grok",
  user: "b@example.com",
  windows: [{ name: "7 days", used: 78, remaining: 22, resetsAt: "2026-11-14T12:09:03Z" }],
};
const deepseekEntry: EngineQuotaEntry = {
  provider: "deepseek",
  name: "DeepSeek",
  windows: [],
  balance: "¥28.30",
};

function snapshotFor(agentId: string, native: Record<string, UsageWindow>, engine: EngineQuotaEntry[]) {
  const state = {
    ...emptySessionUsage(),
    windows: native,
  };
  return buildUsageSnapshot({
    agentId,
    agentLabel: agentId,
    state,
    connected: true,
    engineQuota: engine,
  });
}

const ids = (snap: ReturnType<typeof buildUsageSnapshot>) => snap.windows.map((w) => w.id);

// ── 1. 无原生行：own-family 引擎行出现（此前被整族跳过 = 用户点不出来的根因） ──
console.log("own-family fallback when native probe has nothing");
{
  const snap = snapshotFor("codex", {}, [codexEntry, grokEntry, deepseekEntry]);
  const rowIds = ids(snap);
  ok("codex 5h engine row shown", rowIds.includes("engine:codex:5 hours"));
  ok("codex weekly engine row shown", rowIds.includes("engine:codex:7 days"));
  ok("grok engine row still shown", rowIds.includes("engine:grok:7 days"));
  ok("non-own balance-only entry keeps its balance row", rowIds.includes("engine:deepseek:balance"));
}

// ── 2. 原生行在场：同槽位原生赢，引擎让位 ────────────────────────────────
console.log("native rows claim their slots");
{
  const snap = snapshotFor(
    "codex",
    {
      "five-hour": { id: "five-hour", label: "5-hour limit", percentage: 42, detail: null, kind: "rate_limit" },
      weekly: { id: "weekly", label: "Weekly limit", percentage: 12, detail: null, kind: "rate_limit" },
    },
    [codexEntry, grokEntry],
  );
  const rowIds = ids(snap);
  ok("native five-hour survives", rowIds.includes("five-hour"));
  ok("engine 5 hours suppressed", !rowIds.includes("engine:codex:5 hours"));
  ok("engine 7 days suppressed", !rowIds.includes("engine:codex:7 days"));
  ok("grok rows untouched by codex natives", rowIds.includes("engine:grok:7 days"));
}

// ── 3. 原生只盖住一部分：缺的槽位由引擎补 ────────────────────────────────
console.log("partial native coverage leaves the gap to the engine");
{
  const snap = snapshotFor(
    "codex",
    {
      "five-hour": { id: "five-hour", label: "5-hour limit", percentage: 42, detail: null, kind: "rate_limit" },
    },
    [codexEntry],
  );
  const rowIds = ids(snap);
  ok("engine 5 hours suppressed", !rowIds.includes("engine:codex:5 hours"));
  ok("engine 7 days fills the gap", rowIds.includes("engine:codex:7 days"));
}

// ── 4. 无百分比的原生行（显示层会滤掉）不认领槽位 ────────────────────────
console.log("null-percentage native rows claim nothing");
{
  const snap = snapshotFor(
    "grok",
    {
      weekly: { id: "weekly", label: "Weekly limit", percentage: null, detail: null, kind: "rate_limit" },
    },
    [grokEntry],
  );
  ok("engine grok weekly still shown", ids(snap).includes("engine:grok:7 days"));
}

// ── 5. own-family 余额条目：原生全无时显示，有原生行时让位 ────────────────
console.log("own-family balance-only entry");
{
  const noNative = snapshotFor("grok", {}, [{ ...grokEntry, windows: [], balance: "¥9.90" }]);
  ok("balance shown without native rows", ids(noNative).includes("engine:grok:balance"));
  const withNative = snapshotFor(
    "grok",
    {
      weekly: { id: "weekly", label: "Weekly limit", percentage: 66, detail: null, kind: "rate_limit" },
    },
    [{ ...grokEntry, windows: [], balance: "¥9.90" }],
  );
  ok("balance hidden once natives exist", !ids(withNative).includes("engine:grok:balance"));
}

// ── 6. 主区块：引擎 own-family 组在无原生时顶上 primary ──────────────────
console.log("primary block promotes own-family engine group");
{
  const snap = snapshotFor("codex", {}, [codexEntry, grokEntry]);
  const view = buildQuotaView({ windows: snap.windows, agentLabel: "Codex", agentId: "codex" });
  ok("codex engine group is primary", view.primary?.group.key === "codex");
  ok("primary marked as own subscription", view.primary?.own === true);
  ok("grok stays a dial", view.others.some((g) => g.key === "grok"));

  // 原生在场时 primary 仍是 agent 组（原生数据优先）。
  const nativeSnap = snapshotFor(
    "codex",
    {
      "five-hour": { id: "five-hour", label: "5-hour limit", percentage: 42, detail: null, kind: "rate_limit" },
    },
    [codexEntry],
  );
  const nativeView = buildQuotaView({ windows: nativeSnap.windows, agentLabel: "Codex", agentId: "codex" });
  ok("native agent group wins primary", nativeView.primary?.group.key === "agent");
}

// ── 7. 颜色档位：唯一来源，表盘/明细行/分组底色同一口径 ──────────────────
console.log("quota colour bands (used %)");
{
  // 这些数字是「已用」百分比：92% used = 只剩 8%，必须是红，不是绿。
  const bands: Array<[number, string]> = [
    [0, "ok"],
    [41, "ok"],
    [74.9, "ok"],
    [75, "warn"],
    [78, "warn"],
    [89.9, "warn"],
    [90, "hot"],
    [92, "hot"],
    [100, "hot"],
  ];
  for (const [pct, level] of bands) {
    ok(`pctLevel(${pct}) = ${level}`, pctLevel(pct) === level);
  }
  ok("pctLevel(null) = ok", pctLevel(null) === "ok");

  // 同一条阈值链上的其它出口必须一致（曾是 95/80 与 90/75 两套并存）。
  const meter = (pct: number, kind = "rate_limit") =>
    meterTone({ id: "x", label: "x", percentage: pct, detail: null, kind });
  ok("meterTone(78) = warn", meter(78) === "warn");
  ok("meterTone(92) = hot", meter(92) === "hot");
  ok("meterTone(cost) = none", meter(92, "cost") === "none");

  // 折叠/「Full」只在真耗尽时触发，红色档仍留在盘面上。
  ok("92% used is not exhausted", !isExhausted(92));
  ok("99.4% used is not exhausted", !isExhausted(99.4));
  ok("99.5% used is exhausted", isExhausted(99.5));

  // 分组底色与单项同口径。
  const snap = snapshotFor("codex", {}, [codexEntry]);
  const groups = buildQuotaView({ windows: snap.windows, agentLabel: "Codex", agentId: "codex" });
  const peak = groups.primary?.group.peak ?? null;
  ok(
    "group tone follows pctLevel",
    groups.primary?.group.tone === (peak == null ? "none" : pctLevel(peak)),
  );
}

console.log(`\nverify-quota-fallback: ${passed} checks passed`);
