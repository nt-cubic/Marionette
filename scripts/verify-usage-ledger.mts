/**
 * verify-usage-ledger — 跨会话 Token 总览台账(lib/usageLedger.ts)
 *
 * 1. recordTurnUsageInto:按本地日落账(input/output/cached/reasoning/turns/ms)、
 *    agent 拆分、全 null 的 turnStats 不记、缺 agentId 归 "unknown"。
 * 2. pruneLedger:保留最近 30 个自然日(第 30 天还在、第 31 天走)。
 * 3. summarizeLedger:今天 / 近 7 天 / 近 30 天窗口(本地日边界,纯函数喂 now)。
 * 4. topAgents7:按输出降序、过滤零轮、limit。
 * 5. en 词典直查(脚本跑在 node,默认 zh-CN)。
 *
 * 跑法:npx tsx scripts/verify-usage-ledger.mts
 */

import {
  dayKeyOf,
  pruneLedger,
  recordTurnUsageInto,
  summarizeLedger,
  topAgents7,
  type UsageLedger,
} from "../src/lib/usageLedger";
import { en } from "../src/locales/en";
import type { TurnStats } from "../src/lib/types";

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

function stats(partial: Partial<TurnStats>): TurnStats {
  return { input: null, output: null, cached: null, reasoning: null, total: null, ...partial };
}

// 2026-10-31 是周六,本地时区无关(dayKey 用本地钟,测试用同一把钟)。
const NOW = new Date(2026, 9, 31, 18, 0, 0);
const day = (offsetDays: number): Date => {
  const d = new Date(NOW);
  d.setDate(d.getDate() - offsetDays);
  return d;
};

check("dayKeyOf 格式", dayKeyOf(NOW) === "2026-10-31" && dayKeyOf(day(1)) === "2026-10-30");

// ── 1. 落账 ────────────────────────────────────────────────────────────
{
  let ledger: UsageLedger = { version: 1, days: {} };
  ledger = recordTurnUsageInto(
    ledger,
    { agentId: "claude-code", stats: stats({ input: 1000, output: 500, cached: 2000, reasoning: 100, durationMs: 30_000 }), endedAt: day(0).getTime() },
    NOW,
  );
  ledger = recordTurnUsageInto(
    ledger,
    { agentId: "claude-code", stats: stats({ input: 800, output: 300, durationMs: 12_000 }), endedAt: day(0).getTime() },
    NOW,
  );
  ledger = recordTurnUsageInto(
    ledger,
    { agentId: "grok", stats: stats({ input: 100, output: 50 }), endedAt: day(0).getTime() },
    NOW,
  );
  const d0 = ledger.days["2026-10-31"]!;
  check("同日多轮求和", d0.input === 1900 && d0.output === 850 && d0.cached === 2000 && d0.reasoning === 100);
  check("轮数与流式时长", d0.turns === 3 && d0.ms === 42_000);
  check("agent 拆分", d0.agents["claude-code"]!.output === 800 && d0.agents["grok"]!.output === 50);

  // 全 null 不记
  const untouched = recordTurnUsageInto(ledger, { stats: stats({}), endedAt: day(0).getTime() });
  check("全 null turnStats 不记账", untouched === ledger);
  // 缺 agentId → unknown
  const withUnknown = recordTurnUsageInto(ledger, { stats: stats({ output: 1 }), endedAt: day(0).getTime() });
  check("缺 agentId 归 unknown", withUnknown.days["2026-10-31"]!.agents["unknown"]?.turns === 1);
  // 不同日分桶
  const twoDays = recordTurnUsageInto(ledger, { agentId: "grok", stats: stats({ output: 9 }), endedAt: day(3).getTime() });
  check("不同日分桶", twoDays.days["2026-10-28"]?.output === 9 && twoDays.days["2026-10-31"]?.output === 850);
}

// ── 2. 修剪 ────────────────────────────────────────────────────────────
{
  let ledger: UsageLedger = { version: 1, days: {} };
  for (let i = 0; i <= 40; i += 1) {
    ledger = recordTurnUsageInto(ledger, { agentId: "a", stats: stats({ output: 1 }), endedAt: day(i).getTime() });
  }
  const pruned = pruneLedger(ledger, NOW);
  const keys = Object.keys(pruned.days).sort();
  check("修剪后剩 30 天", keys.length === 30, String(keys.length));
  check("第 30 天保留、第 31 天移除", keys[0] === dayKeyOf(day(29)) && pruned.days[dayKeyOf(day(30))] === undefined);
}

// ── 3. 聚合窗口 ────────────────────────────────────────────────────────
{
  let ledger: UsageLedger = { version: 1, days: {} };
  const seed = (offset: number, output: number, turns = 1) => {
    for (let i = 0; i < turns; i += 1) {
      ledger = recordTurnUsageInto(ledger, { agentId: "a", stats: stats({ output, input: output * 2 }), endedAt: day(offset).getTime() });
    }
  };
  seed(0, 100); // 今天
  seed(3, 200, 2); // 近 7 天
  seed(10, 400); // 近 30 天
  seed(40, 9999); // 窗口外
  const s = summarizeLedger(ledger, NOW);
  check("今天窗口", s.today.output === 100 && s.today.turns === 1 && s.today.input === 200);
  check("近 7 天窗口", s.last7.output === 500 && s.last7.turns === 3);
  check("近 30 天窗口", s.last30.output === 900 && s.last30.turns === 4);
  check("窗口外不混入", s.last30.output !== 900 + 9999);

  const agents = topAgents7(s);
  check("topAgents7 只含 7 天内的 agent", agents.length === 1 && agents[0]!.output === 500);
  const multi: UsageLedger = {
    version: 1,
    days: {
      "2026-10-31": { input: 0, output: 5, cached: 0, reasoning: 0, turns: 1, ms: 0, agents: { b: { output: 5, turns: 1 }, a: { output: 50, turns: 2 } } },
      "2026-10-30": { input: 0, output: 7, cached: 0, reasoning: 0, turns: 1, ms: 0, agents: { c: { output: 7, turns: 1 }, a: { output: 1, turns: 1 } } },
    },
  };
  const ranked = topAgents7(summarizeLedger(multi, NOW));
  check("按输出降序", ranked[0]!.agentId === "a" && ranked[0]!.output === 51 && ranked[1]!.agentId === "c");
}

// ── 4. en 词典 ─────────────────────────────────────────────────────────
{
  check("en:title/hint", en["usageLedger.title"] === "Token totals" && en["usageLedger.hint"].includes("nothing is sent"));
  check("en:单复数齐备", en["usageLedger.turns_one"] === "{n} turn" && en["usageLedger.turns_other"] === "{n} turns");
}

console.log(failures === 0 ? "\n全部通过" : `\n${failures} 项失败`);
process.exit(failures === 0 ? 0 : 1);
