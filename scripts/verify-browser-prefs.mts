import {
  nextBrowserIdleMinutes,
  parseBrowserIdleMinutes,
} from "../src/lib/browserPrefs.ts";

function ok(condition: unknown, label: string) {
  if (!condition) throw new Error(label);
  console.log("  ok  " + label);
}

ok(parseBrowserIdleMinutes(undefined) === 10, "missing preference defaults to 10 minutes");
ok(parseBrowserIdleMinutes(0) === 0, "zero means never");
ok(parseBrowserIdleMinutes(30) === 30, "allowed minute value survives");
ok(parseBrowserIdleMinutes(7) === 10, "invalid values fall back safely");
ok(nextBrowserIdleMinutes(10) === 30, "10 cycles to 30");
ok(nextBrowserIdleMinutes(30) === 60, "30 cycles to 60");
ok(nextBrowserIdleMinutes(60) === 0, "60 cycles to never");
ok(nextBrowserIdleMinutes(0) === 10, "never cycles back to 10");

console.log("\nall checks passed");
