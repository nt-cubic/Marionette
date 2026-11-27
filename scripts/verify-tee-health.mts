/**
 * P0-3 focused checks: the worker tee-loss notice is a pure fold over durable
 * counters, and the recovery action's start cursor is duplicate-safe.
 *
 * Metrics persistence/readback lives in Rust (`cargo test --bin marionette
 * tee`) — this script deliberately stays DOM-free and Tauri-free.
 *
 * Run: npx tsx scripts/verify-tee-health.mts
 */
import {
  TEE_RETRY_SEGMENT,
  teeNoticeCopy,
  teeNoticeState,
  teeRetryStartCursor,
  type ChiefTeeHealth,
} from "../src/lib/chiefTee.ts";

let failures = 0;
function check(name: string, condition: boolean, detail = ""): void {
  if (condition) {
    console.log(`ok   ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL ${name}${detail ? ` -- ${detail}` : ""}`);
  }
}

function health(overrides: Partial<ChiefTeeHealth> = {}): ChiefTeeHealth {
  return {
    sessionId: "session_worker",
    fileLines: 100,
    enqueued: 140,
    written: 100,
    dropped: 40,
    badLines: 0,
    consumedSeq: 60,
    replayStartSeq: 60,
    replayEvents: 40,
    replayTruncated: false,
    registered: false,
    updatedAt: 1,
    ...overrides,
  };
}

// 1. No drops -> no notice at all.
const quiet = teeNoticeState(health({ dropped: 0 }));
check("no drops hides the notice", quiet.visible === false);
check("no drops still reports ledger size", quiet.kept === 100);

// 2. Drops + unread ledger -> visible, recoverable remainder.
const partial = teeNoticeState(health());
check("drops show the notice", partial.visible === true);
check("lost count comes from dropped", partial.lost === 40);
check("remaining = ledger lines - consumed", partial.remaining === 40);
check("durable ledger makes retry eligible", partial.canRetry === true);

// 3. Everything already replayed -> still offers a verification replay.
const consumed = teeNoticeState(health({ consumedSeq: 100 }));
check("fully consumed notice stays visible", consumed.visible === true);
check("fully consumed remaining is zero", consumed.remaining === 0);
check("saved records remain retryable", consumed.canRetry === true);
const consumedCopy = teeNoticeCopy(consumed);
check(
  "consumed copy says all records are present",
  consumedCopy.remainingKey === "session.teeLossConsumed",
);
check(
  "consumed copy keeps the retry action",
  consumedCopy.retryKey === "session.teeLossRetry",
);

// 4. No durable ledger at all -> no false recovery promise.
const noLedger = teeNoticeState(health({ fileLines: 0, consumedSeq: 0 }));
check("no ledger disables retry", noLedger.canRetry === false);
const noLedgerCopy = teeNoticeCopy(noLedger);
check(
  "no ledger copy names the missing records",
  noLedgerCopy.retryKey === "session.teeLossNoRetry",
);
check(
  "body always says dropped content is unrecoverable",
  noLedgerCopy.bodyKey === "session.teeLossBody",
);

// 5. Copy is content-free counts/keys only; bad lines get their own line.
const withBad = teeNoticeCopy(teeNoticeState(health({ badLines: 3 })));
check("copy carries the dropped count", withBad.bodyParams.lost === 40);
check("copy carries the kept count", withBad.bodyParams.kept === 100);
check(
  "bad lines only appear when present",
  withBad.badLinesKey === "session.teeLossBadLines" &&
    withBad.badLinesParams.bad === 3,
);
check(
  "clean copy has no bad-line line",
  teeNoticeCopy(teeNoticeState(health())).badLinesKey === null,
);
check(
  "copy has no free text (keys only)",
  Object.values(withBad).every(
    (value) => typeof value !== "string" || value.startsWith("session."),
  ),
);

// 6. Recovery cursor: empty rail rebuilds from 0; otherwise resume past the
// flushed cursor. Repeated retries never restart from 0 on a populated rail.
check("empty rail rebuilds from seq 0", teeRetryStartCursor(60, 0) === 0);
check("populated rail resumes at the cursor", teeRetryStartCursor(60, 12) === 60);
check("missing cursor clamps to 0", teeRetryStartCursor(-5, 12) === 0);
check(
  "fractional cursors never become fractional seqs",
  teeRetryStartCursor(60.9, 12) === 60,
);

// 7. Segment size stays bounded (no queue growth, no giant allocation).
check(
  "retry segment is bounded",
  Number.isInteger(TEE_RETRY_SEGMENT) &&
    TEE_RETRY_SEGMENT > 0 &&
    TEE_RETRY_SEGMENT <= 2000,
);

if (failures > 0) {
  console.error(`\n${failures} check(s) failed`);
  process.exit(1);
}
console.log("\nall tee-health checks passed");
