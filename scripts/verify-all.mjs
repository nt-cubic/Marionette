/**
 * Aggregate runner for the repo's ad-hoc verification scripts (audit P1-E).
 * Runs every script that works offline; scripts needing a live UI, a real
 * engine binary, or an installed zcode-acp-server are excluded on purpose.
 *
 * Usage: npm run verify
 */
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const dir = path.dirname(fileURLToPath(import.meta.url));

// Needs a running UI with CDP on 9222 / a real engine binary / npm-installed adapter.
const EXCLUDE = new Set([
  "check-ui.mjs",
  "verify-model-source-engine.mjs",
  "patch-zcode-acp-personal-models.mjs",
  "verify-all.mjs",
]);

const scripts = readdirSync(dir)
  .filter((f) => (f.endsWith(".mts") || f.endsWith(".mjs")) && !EXCLUDE.has(f))
  .sort();

const failed = [];
for (const script of scripts) {
  // .mjs runs on plain Node; .mts scripts import ../src/* without extensions,
  // which native ESM resolution rejects - they need tsx (as scripts/README says).
  // npx is a .cmd shim, so on Windows it must go through a shell - with every
  // argument quoted, or spaces in the node path break the command line.
  const scriptPath = path.join(dir, script);
  let run;
  if (script.endsWith(".mts")) {
    run = spawnSync(`npx tsx "${scriptPath}"`, {
      stdio: ["ignore", "inherit", "inherit"],
      shell: true,
      env: process.env,
    });
  } else {
    run = spawnSync(process.execPath, [scriptPath], {
      stdio: ["ignore", "inherit", "inherit"],
      env: process.env,
    });
  }
  const ok = run.status === 0 && !run.error;
  console.log(`\n[verify] ${ok ? "PASS" : "FAIL"} ${script}`);
  if (run.error) console.error(`[verify] spawn error: ${run.error.message}`);
  if (!ok) failed.push(script);
}

console.log(`\n[verify] ${scripts.length - failed.length}/${scripts.length} scripts passed.`);
if (failed.length > 0) {
  console.error(`[verify] failed: ${failed.join(", ")}`);
  process.exit(1);
}
