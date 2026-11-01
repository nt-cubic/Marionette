/**
 * verify-usage-ledger — 跨会话 Token 总览台账(lib/usageLedger.ts + tokenPrices.ts)
 *
 * 1. recordTurnUsageInto:按本地日落账(input/output/cached/cacheCreation/
 *    reasoning/turns/流式时长)、agent 与 model 拆分、小时桶、全 null 的
 *    turnStats 不记、缺 agentId 归 "unknown"。
 * 2. pruneLedger:保留最近 180 个自然日(第 180 天还在、第 181 天走)。
 * 3. summarizeLedger:今天 / 近 7 天 / 近 30 天 + prev7/prev30 上一周期。
 * 4. topAgents7 / rankedModels(>limit 折叠「其它」)。
 * 5. dailySeries / foldHourWeek / foldHourDays / mondayFirst / trendSeries /
 *    cacheHitRate(排除输出) / hourRampStep。
 * 6. tokenPrices:目录解析、厂商优先(pickRate)、命名空间剥离匹配
 *    (marionette\、marionette::、vendor/)、按桶计价与未计价覆盖率。
 * 7. en 词典直查(脚本跑在 node,默认 zh-CN;占位符必须是 {count})。
 *
 * 跑法:npx tsx scripts/verify-usage-ledger.mts
 */

import {
  aggregateDays,
  cacheHitRate,
  dailySeries,
  dayKeyOf,
  deltaPercent,
  foldHourDays,
  foldHourWeek,
  freshTokens,
  hourRampStep,
  mondayFirstHourGrid,
  peakHourOf,
  pruneLedger,
  rankedAgents,
  rankedModels,
  recordTurnUsageInto,
  summarizeLedger,
  topAgents7,
  totalTokens,
  trendSeries,
  type UsageLedger,
} from "../src/lib/usageLedger";
import {
  buildRateIndex,
  costOfBuckets,
  formatUsd,
  pickRate,
  rateForModel,
} from "../src/lib/tokenPrices";
import {
  buildHeatModel,
  heatGridPos,
  heatWeekdayLabels,
} from "../src/lib/activityHeatmap";
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
  return {
    input: null,
    output: null,
    cached: null,
    cacheCreation: null,
    reasoning: null,
    total: null,
    ...partial,
  };
}

// 2026-10-31 是周六,本地时区无关(dayKey 用本地钟,测试用同一把钟)。
const NOW = new Date(2026, 9, 31, 18, 0, 0);
const day = (offsetDays: number, hour = 12): Date => {
  const d = new Date(NOW);
  d.setDate(d.getDate() - offsetDays);
  d.setHours(hour, 0, 0, 0);
  return d;
};

check("dayKeyOf 格式", dayKeyOf(NOW) === "2026-10-31" && dayKeyOf(day(1)) === "2026-10-30");

// ── 1. 落账 ────────────────────────────────────────────────────────────
{
  let ledger: UsageLedger = { version: 1, days: {} };
  ledger = recordTurnUsageInto(
    ledger,
    {
      agentId: "claude-code",
      modelId: "claude-sonnet-4-5",
      stats: stats({ input: 1000, output: 500, cached: 2000, cacheCreation: 300, reasoning: 100, durationMs: 30_000 }),
      endedAt: day(0, 10).getTime(),
    },
  );
  ledger = recordTurnUsageInto(
    ledger,
    {
      agentId: "claude-code",
      modelId: "claude-sonnet-4-5",
      stats: stats({ input: 800, output: 300, durationMs: 12_000 }),
      endedAt: day(0, 11).getTime(),
    },
  );
  ledger = recordTurnUsageInto(
    ledger,
    { agentId: "grok", modelId: "grok-4", stats: stats({ input: 100, output: 50 }), endedAt: day(0, 18).getTime() },
  );
  const d0 = ledger.days["2026-10-31"]!;
  check(
    "同日多轮四桶求和",
    d0.input === 1900 && d0.output === 850 && d0.cached === 2000 && d0.cacheCreation === 300 && d0.reasoning === 100,
  );
  check("轮数与流式时长", d0.turns === 3 && d0.ms === 42_000);
  check("agent 拆分", d0.agents["claude-code"]!.output === 800 && d0.agents["grok"]!.output === 50);
  check(
    "model 拆分",
    d0.models["claude-sonnet-4-5"]!.output === 800 && d0.models["grok-4"]!.input === 100,
  );
  check("小时桶按落账钟点", d0.hours[10] === 1 && d0.hours[11] === 1 && d0.hours[18] === 1 && d0.hours[0] === 0);

  // 全 null 不记
  const untouched = recordTurnUsageInto(ledger, { stats: stats({}), endedAt: day(0).getTime() });
  check("全 null turnStats 不记账", untouched === ledger);
  // 缺 agentId → unknown
  const withUnknown = recordTurnUsageInto(ledger, { stats: stats({ output: 1 }), endedAt: day(0).getTime() });
  check("缺 agentId 归 unknown", withUnknown.days["2026-10-31"]!.agents["unknown"]?.turns === 1);
  // 无 modelId 只进总账不进模型表
  check("缺 modelId 不记模型行", withUnknown.days["2026-10-31"]!.models["grok-4"]!.turns === 1);
  // 不同日分桶
  const twoDays = recordTurnUsageInto(ledger, { agentId: "grok", stats: stats({ output: 9 }), endedAt: day(3).getTime() });
  check("不同日分桶", twoDays.days["2026-10-28"]?.output === 9 && twoDays.days["2026-10-31"]?.output === 850);
}

// ── 2. 修剪(180 天保留) ───────────────────────────────────────────────
{
  let ledger: UsageLedger = { version: 1, days: {} };
  for (let i = 0; i <= 200; i += 1) {
    ledger = recordTurnUsageInto(ledger, { agentId: "a", stats: stats({ output: 1 }), endedAt: day(i).getTime() });
  }
  const pruned = pruneLedger(ledger, NOW);
  const keys = Object.keys(pruned.days).sort();
  check("修剪后剩 180 天", keys.length === 180, String(keys.length));
  check("第 180 天保留、第 181 天移除", keys[0] === dayKeyOf(day(179)) && pruned.days[dayKeyOf(day(180))] === undefined);
}

// ── 3. 聚合窗口 + 上一周期 ────────────────────────────────────────────
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
      "2026-10-31": { input: 0, output: 5, cached: 0, cacheCreation: 0, reasoning: 0, turns: 1, ms: 0, agents: { b: { output: 5, turns: 1 }, a: { output: 50, turns: 2 } }, models: {}, hours: new Array(24).fill(0) },
      "2026-10-30": { input: 0, output: 7, cached: 0, cacheCreation: 0, reasoning: 0, turns: 1, ms: 0, agents: { c: { output: 7, turns: 1 }, a: { output: 1, turns: 1 } }, models: {}, hours: new Array(24).fill(0) },
    },
  };
  const ranked = topAgents7(summarizeLedger(multi, NOW));
  check("按输出降序", ranked[0]!.agentId === "a" && ranked[0]!.output === 51 && ranked[1]!.agentId === "c");

  // 环比窗口:第 8-14 天属于 prev7
  let deltaLedger: UsageLedger = { version: 1, days: {} };
  deltaLedger = recordTurnUsageInto(deltaLedger, { agentId: "a", stats: stats({ output: 150 }), endedAt: day(0).getTime() });
  deltaLedger = recordTurnUsageInto(deltaLedger, { agentId: "a", stats: stats({ output: 100 }), endedAt: day(10).getTime() });
  const ds = summarizeLedger(deltaLedger, NOW);
  check("prev7 窗口", ds.prev7.output === 100);
  check("deltaPercent", deltaPercent(150, 100) === 0.5 && deltaPercent(100, 0) === null);
}

// ── 4. 序列 / 热图折叠 ─────────────────────────────────────────────────
{
  let ledger: UsageLedger = { version: 1, days: {} };
  ledger = recordTurnUsageInto(ledger, { agentId: "a", stats: stats({ output: 10 }), endedAt: day(2, 9).getTime() });
  const series = dailySeries(ledger, 7, NOW);
  check("序列长度与空日", series.length === 7 && series.filter((p) => p.turns === 0).length === 6);
  check(
    "序列 weekday(2026-10-29 是周四)",
    series.find((p) => p.key === "2026-10-29")!.weekday === 4,
  );
  const grid = foldHourWeek(ledger, 1, NOW);
  check("小时×星期折叠", grid[4]![9] === 1 && grid.reduce((s, r) => s + r.reduce((a, b) => a + b, 0), 0) === 1);
  const byDays = foldHourDays(ledger, 7, NOW);
  check("foldHourDays 与 foldHourWeek 同窗", byDays[4]![9] === 1);
  const monday = mondayFirstHourGrid(grid);
  check("周一首行", monday[3]![9] === 1 && monday[6] === grid[0]);
  check("peakHourOf", peakHourOf(grid) === 9);
}

// ── 4b. 缓存命中 / 趋势收桶 / 平方根分档 ─────────────────────────────
{
  check(
    "cacheHitRate 排除输出",
    cacheHitRate({ input: 70, cached: 20, cacheCreation: 10 }) === 0.2,
  );
  check("cacheHitRate 无上下文", cacheHitRate({ input: 0, cached: 0, cacheCreation: 0 }) === null);
  check(
    "freshTokens = 合计 − 缓存读",
    freshTokens({ input: 70, output: 5, cached: 20, cacheCreation: 10 }) === 85,
  );
  check("totalTokens 四桶", totalTokens({ input: 1, output: 2, cached: 3, cacheCreation: 4 }) === 10);

  let ledger: UsageLedger = { version: 1, days: {} };
  ledger = recordTurnUsageInto(ledger, {
    agentId: "a",
    stats: stats({ input: 10, output: 2, cached: 8 }),
    endedAt: day(0).getTime(),
  });
  ledger = recordTurnUsageInto(ledger, {
    agentId: "a",
    stats: stats({ input: 4, output: 1, cached: 0 }),
    endedAt: day(1).getTime(),
  });
  const daily = dailySeries(ledger, 7, NOW);
  const days = trendSeries(daily, "day");
  check("趋势按天长度", days.length === 7);
  const weeks = trendSeries(daily, "week");
  check("趋势按周收桶", weeks.length >= 1 && weeks.length <= 2);
  check(
    "按周合计守恒",
    weeks.reduce((s, p) => s + p.cache + p.fresh, 0) ===
      days.reduce((s, p) => s + p.cache + p.fresh, 0),
  );

  const agents = rankedAgents(
    {
      ...aggregateDays(ledger, 7, NOW),
    },
    7,
  );
  check("rankedAgents 有本助手", agents.some((row) => row.agentId === "a"));

  check("hourRampStep 零", hourRampStep(0, 100) === null);
  check("hourRampStep 满档", hourRampStep(100, 100) === 6);
  check("hourRampStep 平方根中档", hourRampStep(25, 100) === 3);
}

// ── 5. rankedModels 折叠其它 ───────────────────────────────────────────
{
  const agg = aggregateDays(
    {
      version: 1,
      days: {
        "2026-10-31": {
          input: 10,
          output: 10,
          cached: 0,
          cacheCreation: 0,
          reasoning: 0,
          turns: 9,
          ms: 0,
          agents: {},
          hours: new Array(24).fill(0),
          models: Object.fromEntries(
            Array.from({ length: 9 }, (_, i) => [
              `m${i}`,
              { input: 10 - i, output: 0, cached: 0, cacheCreation: 0, turns: 1 },
            ]),
          ),
        },
      },
    },
    1,
    NOW,
  );
  const rows = rankedModels(agg, 6);
  check("模型 >limit 折叠其它", rows.length === 7 && rows[rows.length - 1]!.modelId === "other");
  check("模型按总量降序", rows[0]!.modelId === "m0" && rows[0]!.input === 10);
}

// ── 6. 牌价目录(models.dev 形状) ────────────────────────────────────────
{
  const catalog = {
    anthropic: {
      models: {
        "claude-sonnet-4-5": { name: "Claude Sonnet 4.5", cost: { input: 3, output: 15, cache_read: 0.3, cache_write: 3.75 } },
      },
    },
    openrouter: {
      models: {
        "anthropic/claude-sonnet-4-5": { name: "Claude Sonnet 4.5 (OpenRouter)", cost: { input: 3.3, output: 16.5 } },
      },
    },
    deepseek: {
      models: {
        "deepseek-chat": { name: "DeepSeek V3", cost: { input: 0.27, output: 1.1, cache_read: 0.07 } },
      },
    },
    somehost: {
      models: {
        "unpriced-model": { name: "No Cost Field" },
        "half-priced": { name: "Half", cost: { input: 1 } },
      },
    },
  };
  const index = buildRateIndex(catalog, Date.now());
  // 厂商自家压过转售价
  const claude = rateForModel("anthropic/claude-sonnet-4-5", index);
  check("provider/model 精确命中", claude?.providerId === "anthropic" && claude?.output === 15);
  check(
    "厂商优先于转售价",
    pickRate(index.byModel.get("claude-sonnet-4-5")!)?.providerId === "anthropic",
  );
  // 命名空间剥离:zcode 桥 marionette\、dsh 桥 marionette::、裸 id
  check(
    "命名空间剥离(marionette\\ 与 ::)",
    rateForModel("marionette\\claude-sonnet-4-5", index)?.input === 3 &&
      rateForModel("marionette::deepseek/deepseek-chat", index)?.providerId === "deepseek",
  );
  // 未收录
  check("未收录返回 null", rateForModel("totally-unknown", index) === null);
  // 按桶计价 + 覆盖率
  const cost = costOfBuckets(
    { input: 1_000_000, output: 1_000_000, cached: 1_000_000, cacheCreation: 1_000_000 },
    index.byProviderModel.get("anthropic/claude-sonnet-4-5")![0]!,
  );
  check(
    "四桶计价",
    Math.abs(cost.usd! - (3 + 15 + 0.3 + 3.75)) < 1e-9 && cost.unpricedTokens === 0,
  );
  const halfCost = costOfBuckets(
    { input: 1_000_000, output: 1_000_000, cached: 0, cacheCreation: 0 },
    index.byModel.get("half-priced")![0]!,
  );
  check("未发布桶不计价、覆盖率如实", Math.abs(halfCost.usd! - 1) < 1e-9 && halfCost.unpricedTokens === 1_000_000);
  check("无价模型 usd=null", costOfBuckets({ input: 5, output: 5, cached: 0, cacheCreation: 0 }, null).usd === null);
  check("金额格式", formatUsd(12.5) === "$12.50" && formatUsd(0.000481).startsWith("$0.000481"));
}

// ── 7. 活动格子模型(lib/activityHeatmap.ts,骨架学 uiarc) ─────────────
{
  // 2026-10-25(周日)起 14 天,周一开头 → lead=6,首列只有 1 格。
  const days = Array.from({ length: 14 }, (_, i) => {
    const d = new Date(2026, 9, 25 + i);
    const pad = (n: number) => String(n).padStart(2, "0");
    return {
      key: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`,
      value: i === 0 ? 400 : i % 3 === 0 ? 100 : i % 3 === 1 ? 50 : 0,
    };
  });
  const model = buildHeatModel(days, { weekStartsOn: 1, locale: "zh-CN" });
  check("热图:首周 lead 补齐(周一起)", model.lead === 6 && model.weeks === 3, `lead=${model.lead} weeks=${model.weeks}`);
  check("热图:季度阈值", model.thresholds[0] === 100 && model.thresholds[1] === 200 && model.thresholds[2] === 300);
  check("热图:等级与 perLevel", model.levels[0] === 4 && model.perLevel.reduce((a, b) => a + b, 0) === 14);
  check("热图:稀疏日摊平 + 总量", model.counts[1] === 50 && model.counts[2] === 0 && model.total === 400 + 100 * 4 + 50 * 5);
  check("热图:startKey 是本地日(不是 UTC 折叠)", model.startKey === "2026-10-25");
  check(
    "热图:网格位 index→列行",
    heatGridPos(0, 6).col === 0 && heatGridPos(0, 6).row === 6 && heatGridPos(1, 6).col === 1 && heatGridPos(1, 6).row === 0,
  );
  const labels = heatWeekdayLabels(1, "zh-CN");
  check(
    "热图:星期标只标一/三/五",
    labels[0] !== "" && labels[1] === "" && labels[2] !== "" && labels[3] === "" && labels[4] !== "" && labels[5] === "" && labels[6] === "",
  );
  // 月标:2026-09-01(周二)起 70 天,lead=1,Oct-1 落在第 4 列,隔 ≥3 列 → 两标都留。
  const mkDays = (from: Date, n: number) =>
    Array.from({ length: n }, (_, i) => {
      const d = new Date(from.getFullYear(), from.getMonth(), from.getDate() + i);
      const pad = (x: number) => String(x).padStart(2, "0");
      return {
        key: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`,
        value: 10,
      };
    });
  const longModel = buildHeatModel(mkDays(new Date(2026, 8, 1), 70), { weekStartsOn: 1, locale: "zh-CN" });
  check(
    "热图:月标隔 ≥3 列都保留",
    longModel.months.length === 3 && longModel.months[0]!.col === 0 && longModel.months[1]!.col === 4 && longModel.months[2]!.col === 8,
    JSON.stringify(longModel.months),
  );
  // 残月让位:2026-09-28(周一)起,Oct-1 与首标同列 → 9 月残标被丢,只剩 10 月。
  const stubModel = buildHeatModel(mkDays(new Date(2026, 8, 28), 30), { weekStartsOn: 1, locale: "zh-CN" });
  check(
    "热图:残月标签让位",
    stubModel.months.length === 1 && stubModel.months[0]!.col === 0 && stubModel.months[0]!.label.includes("10"),
    JSON.stringify(stubModel.months.map((m) => m.label)),
  );
}

// ── 8. en 词典(占位符 {count}:t() 只认 params 里的键) ──────────────────
{
  check(
    "en:title/viewAnalysis",
    en["usageLedger.title"] === "Token totals" && en["usageLedger.viewAnalysis"] === "Analyze",
  );
  check(
    "en:单复数用 {count}",
    en["usageLedger.turns_one"] === "{count} turn" && en["usageLedger.turns_other"] === "{count} turns",
  );
  check("en:费用文案非实付声明", en["usageLedger.costNote"].includes("not what your plan paid"));
}

console.log(failures === 0 ? "\n全部通过" : `\n${failures} 项失败`);
process.exit(failures === 0 ? 0 : 1);
