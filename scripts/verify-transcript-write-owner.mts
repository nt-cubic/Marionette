/**
 * P0-2 focused checks: the canonical transcript has exactly one writer, and
 * tee replay flushes before the cursor advances.
 *
 * These are pure-logic tests (no DOM, no Rust). The storage/tee halves are
 * covered by `cargo test --bin marionette` (`cursor_is_monotonic_and_replay_stays_append_only`,
 * `canonical_append_after_load_keeps_prefix_and_adds_only_suffix`).
 */
import {
  planTranscriptPersist,
  replayStartCursor,
} from "../src/app/useSessionRails.ts";
import {
  teeEventsAfterCursor,
  uncoveredTeeEvents,
  type ChiefTeeEvent,
} from "../src/lib/chiefTee.ts";
import type { SessionEvent } from "../src/lib/types.ts";

let failures = 0;
function check(name: string, condition: boolean, detail = ""): void {
  if (condition) {
    console.log(`ok   ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL ${name}${detail ? ` -- ${detail}` : ""}`);
  }
}

const ev = (id: string): SessionEvent =>
  ({ id, type: "assistant_message", sessionId: "s", text: id }) as unknown as SessionEvent;

// 1. Nothing to write.
check(
  "empty rail plans none",
  planTranscriptPersist([], undefined, { headTruncated: false }).kind === "none",
);

// 2. Reference-identical prefix + growth appends only the suffix (once).
const a = ev("a");
const b = ev("b");
const c = ev("c");
const loaded = [a, b];
const grown = [a, b, c];
const appendPlan = planTranscriptPersist(grown, loaded, { headTruncated: true });
check(
  "prefix-identical growth appends only the new suffix",
  appendPlan.kind === "append" &&
    appendPlan.suffix.length === 1 &&
    appendPlan.suffix[0] === c,
);
check(
  "truncated head does not force ensure-head when the prefix is identical",
  appendPlan.kind !== "ensure-head",
);

// 3. A second flush with the same rail is a no-op (exactly once / no dupes).
const afterFirstFlush = planTranscriptPersist(grown, grown, { headTruncated: false });
check("second flush after a durable write plans none", afterFirstFlush.kind === "none");

// 4. A different rail with a truncated head must load the head first.
const different = [b, c];
const differentTruncated = planTranscriptPersist(different, loaded, {
  headTruncated: true,
});
check(
  "different rail + truncated head never rewrites",
  differentTruncated.kind === "ensure-head",
);

// 5. Without a truncated head a different rail is a full rewrite (still via
//    the one writer), never an append of a non-suffix.
const differentFull = planTranscriptPersist(different, loaded, {
  headTruncated: false,
});
check(
  "different rail without truncation rewrites the whole persistable view",
  differentFull.kind === "rewrite" &&
    differentFull.events.length === different.length &&
    differentFull.events[0] === b,
);

// 6. An empty canonical transcript ignores a stale consumed cursor.
check("empty transcript replays from seq 0", replayStartCursor(42, true) === 0);
check("non-empty transcript keeps its cursor", replayStartCursor(42, false) === 42);
check("missing / invalid cursor falls back to 0", replayStartCursor(Number.NaN, false) === 0);

// 7. Repeated opens do not re-apply the same span after a successful flush.
const tee: ChiefTeeEvent[] = [
  { seq: 1, ts: 1000 },
  { seq: 2, ts: 2000 },
];
const firstCursor = replayStartCursor(99, true);
const firstBatch = uncoveredTeeEvents(teeEventsAfterCursor(tee, firstCursor), 0);
check("empty transcript replays the whole raw tee", firstBatch.length === 2);
const consumed = 2; // only after flushSessionTranscript returned true
const secondBatch = uncoveredTeeEvents(teeEventsAfterCursor(tee, consumed), 2000);
check("a reopen after cursor advance replays nothing", secondBatch.length === 0);

// 8. A failed flush must keep the cursor unadvanced, so the next open retries.
const retryBatch = uncoveredTeeEvents(teeEventsAfterCursor(tee, firstCursor), 0);
check("failed flush leaves the same span available for retry", retryBatch.length === 2);

if (failures > 0) {
  console.error(`\nverify-transcript-write-owner: ${failures} failure(s)`);
  process.exitCode = 1;
} else {
  console.log("\nverify-transcript-write-owner: OK");
}
