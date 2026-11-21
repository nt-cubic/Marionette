import assert from "node:assert/strict";
import { runUsageProbe } from "../src/lib/usageRefresh.ts";
import type { SessionStatus } from "../src/lib/types.ts";

let failures = 0;
function check(name: string, fn: () => void | Promise<void>) {
  return Promise.resolve()
    .then(fn)
    .then(() => console.log(`  ok  ${name}`))
    .catch((error) => {
      failures += 1;
      console.error(`  FAIL ${name}\n${error instanceof Error ? error.stack : String(error)}`);
    });
}

async function sequence(status: SessionStatus) {
  const calls: string[] = [];
  const outcome = await runUsageProbe({
    status,
    ensureReady: async () => {
      calls.push("warm");
    },
    send: async () => {
      calls.push("send");
    },
  });
  return { outcome, calls };
}

console.log("usage refresh cold-session probe");

for (const status of ["exited", "error", "starting", "waiting"] as const) {
  await check(`${status} warms or validates ACP before sending`, async () => {
    const got = await sequence(status);
    assert.equal(got.outcome, "sent");
    assert.deepEqual(got.calls, ["warm", "send"]);
  });
}

await check("running defers to the post-turn refresh", async () => {
  const got = await sequence("running");
  assert.equal(got.outcome, "deferred");
  assert.deepEqual(got.calls, []);
});

await check("warm failure never sends into a missing process", async () => {
  let sent = false;
  await assert.rejects(
    runUsageProbe({
      status: "exited",
      ensureReady: async () => {
        throw new Error("warm failed");
      },
      send: async () => {
        sent = true;
      },
    }),
    /warm failed/,
  );
  assert.equal(sent, false);
});

await check("send failure propagates to the refresh caller", async () => {
  await assert.rejects(
    runUsageProbe({
      status: "waiting",
      ensureReady: async () => undefined,
      send: async () => {
        throw new Error("send failed");
      },
    }),
    /send failed/,
  );
});

if (failures > 0) {
  console.error(`\nverify-usage-refresh: ${failures} failed`);
  process.exit(1);
}
console.log("\nverify-usage-refresh: 7 checks passed");
