/**
 * Claude `_claude/rateLimit` payload shapes (2026-11-22).
 *
 * 背景：朋友机的诊断报告实锤——claude-agent-acp 0.109.x 的 usage_update 里，
 * 顶层 `utilization` 不再总是出现（70 条事件缺席、只在告警态偶发），利用率
 * 挪进了 `unifiedWindows.{five_hour,seven_day}.utilization`。旧解析只读顶层
 * 字段 → percentage null → isMeaningfulWindow 过滤 → 右栏 CC 用量行整场消失。
 * 本脚本钉住三种形状：unified-only / 顶层+unified 并存 / 顶层-only，以及
 * 「有百分比的行不被无百分比的行顶掉」的合并序。
 *
 * Run: npx tsx scripts/verify-claude-usage-meta.mts
 */
import assert from "node:assert/strict";

import { extractUsageFromAcpData, mergeUsageFromAcp } from "../src/lib/usage.ts";
import { buildQuotaView } from "../src/lib/quotaRail.ts";
import type { UsageWindow } from "../src/lib/types.ts";

let passed = 0;
function ok(name: string, cond: boolean) {
  assert.ok(cond, name);
  passed += 1;
  console.log(`  ✓ ${name}`);
}

function envelope(meta: unknown) {
  return {
    jsonrpc: "2.0",
    method: "session/update",
    params: {
      sessionId: "native-1",
      update: { sessionUpdate: "usage_update", used: 311075, size: 1000000, _meta: meta },
    },
  };
}

function windowOf(windows: UsageWindow[], id: string): UsageWindow | undefined {
  return windows.find((w) => w.id === id);
}

// ── 形状 1：unified-only（0.109.x 常态；诊断报告 22:17 原文） ─────────────
console.log("unifiedWindows-only shape (adapter 0.109.x)");
{
  const data = envelope({
    "_claude/rateLimit": {
      status: "allowed",
      resetsAt: 1795362000,
      rateLimitType: "five_hour",
      overageStatus: "rejected",
      overageDisabledReason: "org_level_disabled",
      isUsingOverage: false,
      unifiedWindows: {
        five_hour: { utilization: 0.47, resetsAt: 1795362000, status: "allowed" },
        seven_day: { utilization: 0.12, resetsAt: 1795874400, status: "allowed" },
      },
    },
    "_claude/model": "claude-sonnet-5-5",
  });
  const got = extractUsageFromAcpData(data);
  ok("extracted", got != null);
  const five = windowOf(got!.rateWindows, "five-hour");
  const weekly = windowOf(got!.rateWindows, "weekly");
  ok("five-hour carries the unified utilization", five?.percentage === 47);
  ok("weekly carries the unified utilization", weekly?.percentage === 12);
  ok("reset hint survived", !!five?.detail && /resets/.test(five.detail));
}

// ── 形状 2：顶层 + unifiedWindows 并存（告警态事件） ───────────────────────
console.log("top-level utilization + unifiedWindows together");
{
  const data = envelope({
    "_claude/rateLimit": {
      status: "allowed_warning",
      resetsAt: 1795339800,
      rateLimitType: "five_hour",
      utilization: 0.98,
      unifiedWindows: {
        five_hour: { utilization: 0.98, resetsAt: 1795339800, status: "allowed_warning", surpassedThreshold: 0.9 },
        seven_day: { utilization: 0.08, resetsAt: 1795874400, status: "allowed" },
      },
    },
    "_claude/model": "claude-opus-5-5",
  });
  const got = extractUsageFromAcpData(data)!;
  const five = windowOf(got.rateWindows, "five-hour");
  ok("percentage kept when both shapes agree", five?.percentage === 98);
  ok("warning status lands in the detail", !!five?.detail && /warning/i.test(five.detail));
  ok("weekly appears from unified even when top-level is five_hour only", windowOf(got.rateWindows, "weekly")?.percentage === 8);
}

// ── 形状 3：顶层-only（旧适配器；不能回归） ────────────────────────────────
console.log("legacy top-level-only shape");
{
  const data = envelope({
    "_claude/rateLimit": {
      status: "allowed_warning",
      resetsAt: 1795339800,
      rateLimitType: "five_hour",
      utilization: 0.99,
    },
  });
  const got = extractUsageFromAcpData(data)!;
  ok("legacy row still parses", windowOf(got.rateWindows, "five-hour")?.percentage === 99);
}

// ── 合并序：有百分比的行不被无百分比的行顶掉 ──────────────────────────────
console.log("merge precedence");
{
  const data = envelope({
    "_claude/rateLimit": {
      status: "allowed",
      rateLimitType: "five_hour",
      utilization: 0.55, // 顶层有数
      unifiedWindows: {
        five_hour: { resetsAt: 1795362000, status: "allowed" }, // unified 无 utilization
      },
    },
  });
  const got = extractUsageFromAcpData(data)!;
  ok("percentage-bearing row keeps its slot", windowOf(got.rateWindows, "five-hour")?.percentage === 55);
}

// ── 端到端：右栏把 CC 认成 own 订阅主块 ────────────────────────────────────
console.log("quota view for a claude-code session");
{
  const data = envelope({
    "_claude/rateLimit": {
      status: "allowed",
      resetsAt: 1795362000,
      rateLimitType: "five_hour",
      unifiedWindows: {
        five_hour: { utilization: 0.47, resetsAt: 1795362000, status: "allowed" },
        seven_day: { utilization: 0.12, resetsAt: 1795874400, status: "allowed" },
      },
    },
  });
  const merged = mergeUsageFromAcp(undefined, data)!;
  const view = buildQuotaView({
    windows: Object.values(merged.windows),
    agentLabel: "Claude Code",
    agentId: "claude-code",
    modelId: "sonnet",
  });
  ok("primary is the agent's own subscription", view.primary?.own === true && view.primary.group.key === "agent");
  ok("both windows render in the primary group", view.primary?.group.windows.length === 2);
  ok("primary peak reads the tightest window", view.primary?.group.peak === 47);
}

// ── 反例：什么数字都没有 → 显示层安静跳过（引擎 own-family 行兜底的场景） ──
console.log("no numbers anywhere stays quiet");
{
  const data = envelope({
    "_claude/rateLimit": { status: "allowed", rateLimitType: "five_hour" },
  });
  const got = extractUsageFromAcpData(data)!;
  ok("no claude row carries a percentage", got.rateWindows.every((w) => w.percentage == null));
  const view = buildQuotaView({
    windows: got.rateWindows,
    agentLabel: "Claude Code",
    agentId: "claude-code",
  });
  ok("nothing renders for the agent", view.primary == null);
}

console.log(`\n${passed} checks passed`);
