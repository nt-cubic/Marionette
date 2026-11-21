/**
 * Composer 偏好漂移检测的回归（capabilityDrift.ts）。
 *
 * 背景（2026-11-21，zcode 1113「余额不足」）：zcode 的账号枚举按 spawn 时的
 * 凭据可用性浮动——一次重启 advertise 的目录里少了 Start Plan 账号，旧版
 * detectCapabilityDrift 把已存偏好 `account:zai-start-plan\…` 当成「不再
 * 提供」清空，随后发送链把回退芯片（空余额的 Individual 账号）落盘固化，
 * 每轮 1113。红线：
 *   1) account: 前缀的模型偏好永远不因「目录里没有」被清掉；
 *   2) 非 account 前缀的普通模型 id 沿用旧行为（清空 + 提示）；
 *   3) mode/effort 漂移行为不变；
 *   4) isAccountPrefTransientlyMissing 是发送链 fail-closed 的唯一判定，
 *      已存账号偏好缺席 → 发送前报错（fail-closed），落盘跳过（不覆盖）。
 *
 * Run: npx tsx scripts/verify-capability-drift.mts
 */
import assert from "node:assert/strict";

import { applyLocale } from "../src/lib/i18n";
import {
  detectCapabilityDrift,
  isAccountPrefTransientlyMissing,
} from "../src/lib/capabilityDrift";
import type { CapabilitySnapshot, SessionComposerPrefs } from "../src/lib/types";

await applyLocale("zh-CN");

let passed = 0;
function ok(name: string, cond: boolean) {
  assert.ok(cond, name);
  passed += 1;
  console.log(`  ✓ ${name}`);
}

// wire 形状里账号与模型之间是一个反斜杠（zcode `account:<plan>\<model>`）。
const START_PLAN = "account:zai-start-plan" + "\\" + "GLM-5.3-Flash";
const INDIVIDUAL = "account:zai-individual-coding-plan" + "\\" + "GLM-5.3";
const INDIVIDUAL_FLASH = "account:zai-individual-coding-plan" + "\\" + "GLM-5.3-Flash";

const caps = (over: Partial<CapabilitySnapshot> = {}): CapabilitySnapshot =>
  ({
    models: [
      { id: INDIVIDUAL, name: "GLM-5.3" },
      { id: INDIVIDUAL_FLASH, name: "GLM-5.3-Flash" },
    ],
    modes: [{ id: "yolo", name: "Yolo" }],
    currentModel: INDIVIDUAL,
    currentMode: "yolo",
    ...over,
  }) as CapabilitySnapshot;

const prefs = (over: Partial<SessionComposerPrefs> = {}): SessionComposerPrefs =>
  ({
    preferredModel: START_PLAN,
    preferredMode: "yolo",
    ...over,
  }) as SessionComposerPrefs;

console.log("account-scoped prefs survive a transient catalog drop");
{
  const drift = detectCapabilityDrift(prefs(), caps());
  ok("no drift raised for a missing account model", drift === null);
}
{
  // 账号模型缺席但 effort 控件在目录里 → 什么都不清
  const drift = detectCapabilityDrift(
    prefs({ preferredEffortId: "max" }),
    caps({ effortOptions: [{ id: "max", name: "Max" }] }),
  );
  ok("no drift when effort control is alive", drift === null);
}
{
  // effort 控件整个消失 → effortId 清（文档化行为），模型偏好仍不清
  const drift = detectCapabilityDrift(prefs({ preferredEffortId: "max" }), caps());
  ok("dead effort control is raised", drift?.issues[0]?.field === "effortId");
  ok("dead effortId is cleared", drift?.clearedPrefs.preferredEffortId === null);
  ok("account model pref survives the effort wipe", drift?.clearedPrefs.preferredModel === undefined);
}

console.log("plain model ids keep the old wipe behavior");
{
  const drift = detectCapabilityDrift(prefs({ preferredModel: "gemini-3.8-flash-high" }), caps());
  ok("drift raised for a dead plain model", drift?.issues[0]?.field === "model");
  ok("plain model pref is cleared", drift?.clearedPrefs.preferredModel === null);
  ok("account mode pref rides along untouched", drift?.clearedPrefs.preferredMode === undefined);
}

console.log("mode drift unchanged");
{
  const drift = detectCapabilityDrift(prefs({ preferredMode: "full-auto" }), caps());
  ok("dead mode is raised", drift?.issues[0]?.field === "mode");
  ok("dead mode is cleared", drift?.clearedPrefs.preferredMode === null);
}
{
  const drift = detectCapabilityDrift(
    prefs({ preferredMode: "full-auto" }),
    caps({ modes: [{ id: "agent-full-access", name: "Full Access" }] }),
    "codex",
  );
  ok("codex legacy mode remaps instead of wiping", drift?.clearedPrefs.preferredMode === "agent-full-access");
}

console.log("isAccountPrefTransientlyMissing is the single send-chain predicate");
{
  ok("missing account pref reports true", isAccountPrefTransientlyMissing(START_PLAN, caps()) === true);
  ok("advertised account pref reports false", isAccountPrefTransientlyMissing(INDIVIDUAL, caps()) === false);
  ok("advertised flash pref reports false", isAccountPrefTransientlyMissing(INDIVIDUAL_FLASH, caps()) === false);
  ok(
    "empty catalog reports false (nothing to compare)",
    isAccountPrefTransientlyMissing("account:x/y", caps({ models: [] })) === false,
  );
  ok("no caps reports false", isAccountPrefTransientlyMissing("account:x/y", null) === false);
  ok("plain id reports false", isAccountPrefTransientlyMissing("gemini-3.8-flash", caps()) === false);
  ok("null pref reports false", isAccountPrefTransientlyMissing(null, caps()) === false);
}

console.log(`\nverify-capability-drift: ${passed} checks passed`);
