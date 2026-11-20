/**
 * verify-composer-prefs — restore job lifetime (always-approve replay)
 *
 * Starting a Grok dialog with "Always approve" saved used to repaint the chip
 * but never replay `/always-approve on`: the restore effect returned a cleanup
 * flag, and any caps/sessionPrefs identity change cancelled the sequence after
 * the model step (always-approve is its last step). This guards the shape that
 * fixed it: the job survives dependency churn, and only a dialog change or a
 * dead agent invalidates it.
 *
 * Run: npx tsx scripts/verify-composer-prefs.mts
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const composer = readFileSync(join(root, "src/components/Composer.tsx"), "utf8");

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

const jobStart = composer.indexOf("const generation = ++restoreGeneration.current;");
const jobEnd = composer.indexOf("}, [caps, capsLive, sessionId, agent.id, sessionPrefs]);", jobStart);
check("restore job is located", jobStart > 0 && jobEnd > jobStart);
const job = jobStart >= 0 && jobEnd > jobStart ? composer.slice(jobStart, jobEnd) : "";

check(
  "restore job uses a generation token (survives caps/prefs churn)",
  job.includes("const generation = ++restoreGeneration.current;") &&
    job.includes("const alive = () => generation === restoreGeneration.current;"),
);

check(
  "restore job has no cleanup flag that can cancel it",
  !job.includes("cancelled"),
);

check(
  "every async phase checks alive()",
  (job.match(/if \(!alive\(\)\) return;/g) ?? []).length >= 2 &&
    (job.match(/if \(alive\(\)\)/g) ?? []).length >= 4,
);

check(
  "always-approve replay is part of the restore job (after model/mode)",
  job.includes("if (aaSpec && prefAlways === true)") &&
    job.includes("commitUpdate({ alwaysApprove: true }") &&
    job.includes("deferredAlwaysApprove.current = true;"),
);

const identityStart = composer.indexOf("HARD RULE: capability state is rebound");
const identityEnd = composer.indexOf("}, [sessionId, agent.id]);", identityStart);
const identity = identityStart >= 0 && identityEnd > identityStart
  ? composer.slice(identityStart, identityEnd)
  : "";
check(
  "dialog identity change invalidates the running job",
  identity.includes("restoreGeneration.current += 1;"),
);
check(
  "unmounting the composer invalidates the running job",
  identity.includes("return () => {") &&
    identity.includes("restoreGeneration.current += 1;"),
);

const deadAgentStart = composer.indexOf("Parent cleared caps (session switch / process died)");
const deadAgentEnd = composer.indexOf("}, [agent.id, capabilitiesProp, applyCaps]);", deadAgentStart);
const deadAgent = deadAgentStart >= 0 && deadAgentEnd > deadAgentStart
  ? composer.slice(deadAgentStart, deadAgentEnd)
  : "";
check(
  "a dead agent process invalidates the running job",
  deadAgent.includes("restoreGeneration.current += 1;"),
);

check(
  "key is claimed before async work (no parallel restores)",
  composer.indexOf("prefsRestoredKey.current = key;") > 0 &&
    composer.indexOf("prefsRestoredKey.current = key;") < jobStart,
);

if (failures > 0) {
  console.error(`\n${failures} check(s) failed`);
  process.exit(1);
}
console.log("\ncomposer pref restore: all checks passed");
