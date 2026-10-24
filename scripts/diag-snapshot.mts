// Simulate the Usage panel snapshot with the LIVE engine quota payload.
import { execFileSync } from "node:child_process";
import { homedir } from "node:os";
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { buildUsageSnapshot } from "../src/lib/usage.ts";
import type { EngineQuotaEntry } from "../src/lib/api.ts";

const exe = join(homedir(), ".marionette", "engine", "magpie.exe");
const raw = execFileSync(exe, ["quota", "--json"], { encoding: "utf8", timeout: 60000 });
const entries = JSON.parse(raw) as Array<Record<string, unknown>>;
const engineQuota: EngineQuotaEntry[] = entries.map((e) => ({
  provider: String(e.provider ?? ""),
  name: String(e.name ?? ""),
  user: typeof e.user === "string" ? e.user : null,
  windows: (Array.isArray(e.windows) ? e.windows : []).map((w: Record<string, unknown>) => ({
    name: String(w.name ?? ""),
    used: typeof w.used === "number" ? w.used : null,
    remaining: typeof w.remaining === "number" ? w.remaining : null,
    resetsAt: typeof w.resetsAt === "string" ? w.resetsAt : null,
  })),
  balance: typeof e.balance === "string" ? e.balance : null,
}));

const out = [];
for (const agentId of ["grok-build", "codex"]) {
  const snap = buildUsageSnapshot({ agentId, agentLabel: agentId, state: undefined, connected: true, engineQuota });
  out.push(`== agentId=${agentId} ==`);
  for (const w of snap.windows) {
    out.push(`  [${w.kind}] ${w.id} | ${w.label} | pct=${w.percentage} | detail=${w.detail}`);
  }
  out.push(`  note=${snap.note}`);
  const codexRows = snap.windows.filter((w) => w.id.includes("codex"));
  out.push(`  codex-related rows: ${codexRows.length}`);
}
writeFileSync("D:\\Projects\\Marionette\\diag-snapshot.out.txt", out.join("\n"), "utf8");
console.log("done");
