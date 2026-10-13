/**
 * Transcript tab-scroll restore smoke.
 *
 * Run: npx tsx scripts/verify-transcript-scroll.mts
 */
import assert from "node:assert/strict";
import {
  applyScrollSnap,
  captureScrollSnap,
  isNearTranscriptBottom,
  TRANSCRIPT_NEAR_BOTTOM_PX,
} from "../src/lib/transcriptScroll.ts";
import {
  resetVirtualSizeCache,
  sizeMapForKey,
} from "../src/lib/virtualSizeCache.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

check("near-bottom uses the chat slack", () => {
  assert.equal(TRANSCRIPT_NEAR_BOTTOM_PX, 80);
  assert.equal(
    isNearTranscriptBottom({ scrollTop: 920, scrollHeight: 1000, clientHeight: 100 }),
    true,
  );
  assert.equal(
    isNearTranscriptBottom({ scrollTop: 100, scrollHeight: 1000, clientHeight: 100 }),
    false,
  );
});

check("capture marks tail vs history", () => {
  const tail = captureScrollSnap({
    scrollTop: 900,
    scrollHeight: 1000,
    clientHeight: 100,
  });
  assert.equal(tail.stickToBottom, true);
  assert.equal(tail.scrollTop, 900);

  const history = captureScrollSnap({
    scrollTop: 40,
    scrollHeight: 4000,
    clientHeight: 800,
  });
  assert.equal(history.stickToBottom, false);
  assert.equal(history.scrollTop, 40);
});

check("missing snap pins to live end (first visit)", () => {
  const el = { scrollTop: 0, scrollHeight: 2400 };
  assert.equal(applyScrollSnap(el, undefined), "bottom");
  assert.equal(el.scrollTop, 2400);
});

check("tail snap re-pins even if estimates grew", () => {
  const el = { scrollTop: 480, scrollHeight: 8000 };
  const mode = applyScrollSnap(el, { stickToBottom: true, scrollTop: 480 });
  assert.equal(mode, "bottom");
  assert.equal(el.scrollTop, 8000);
});

check("history snap restores the pixel offset", () => {
  const el = { scrollTop: 0, scrollHeight: 8000 };
  const mode = applyScrollSnap(el, { stickToBottom: false, scrollTop: 1200 });
  assert.equal(mode, "offset");
  assert.equal(el.scrollTop, 1200);
});

check("size maps survive a tab-key swap", () => {
  resetVirtualSizeCache();
  const a = sizeMapForKey("session-a|clean|v");
  a.set(0, 220);
  a.set(1, 180);
  const b = sizeMapForKey("session-b|clean|v");
  b.set(0, 48);
  const aAgain = sizeMapForKey("session-a|clean|v");
  assert.equal(aAgain, a);
  assert.equal(aAgain.get(0), 220);
  assert.equal(aAgain.get(1), 180);
  resetVirtualSizeCache();
});

console.log(`\n${passed} checks passed`);
