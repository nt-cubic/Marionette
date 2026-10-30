// Quick per-batch gate: every en fragment must cover exactly its zh fragment's keys
// (tsc does the authoritative check once all in-flight batches land).
import { readFileSync } from "node:fs";
const pairs = process.argv.slice(2);
let bad = 0;
for (const zhPath of pairs) {
  const enPath = zhPath.replace(/\.zh\.ts$/, ".en.ts");
  const grab = (src) => [...src.matchAll(/^\s*"([^"]+)"\s*:/gm)].map((m) => m[1]).sort();
  const zhKeys = grab(readFileSync(zhPath, "utf8"));
  const enKeys = grab(readFileSync(enPath, "utf8"));
  const missing = zhKeys.filter((k) => !enKeys.includes(k));
  const extra = enKeys.filter((k) => !zhKeys.includes(k));
  const dup = new Set(enKeys.filter((k, i) => enKeys.indexOf(k) !== i));
  if (missing.length || extra.length || dup.size) {
    bad += 1;
    console.log(`${zhPath}: missing-in-en=[${missing}] extra-in-en=[${extra}] dup=[${[...dup]}]`);
  } else {
    console.log(`${zhPath}: OK (${zhKeys.length} keys)`);
  }
}
process.exit(bad ? 1 : 0);
