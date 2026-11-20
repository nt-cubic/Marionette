/**
 * Transcript tab-scroll restore smoke.
 *
 * Run: npx tsx scripts/verify-transcript-scroll.mts
 */
import assert from "node:assert/strict";
import {
  applyScrollSnap,
  applyTranscriptStickOnScroll,
  applyTranscriptStickOnWheelUp,
  captureScrollAnchor,
  captureScrollSnap,
  isNearTranscriptBottom,
  readTranscriptScrollSnap,
  rememberTranscriptScrollSnap,
  resetTranscriptScrollSnaps,
  resolveScrollAnchorTop,
  TRANSCRIPT_NEAR_BOTTOM_PX,
  TRANSCRIPT_RESTICK_PX,
} from "../src/lib/transcriptScroll.ts";
import { compensateScrollForSizeChange } from "../src/lib/useVirtualWindow.ts";
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

check("anchor restores the visible card instead of a stale pixel offset", () => {
  const anchor = captureScrollAnchor(
    [
      { index: 0, key: "u-a", top: 0 },
      { index: 1, key: "u-b", top: 320 },
      { index: 2, key: "u-c", top: 900 },
    ],
    360,
  );
  assert.deepEqual(anchor, { index: 1, key: "u-b", offset: 40 });
  assert.equal(resolveScrollAnchorTop(anchor, () => 640), 680);
  const el = { scrollTop: 0, scrollHeight: 8_000 };
  assert.equal(applyScrollSnap(el, { stickToBottom: false, scrollTop: 1200 }, 680), "offset");
  assert.equal(el.scrollTop, 680);
});

check("session scroll snapshots survive SessionView unmounts", () => {
  resetTranscriptScrollSnaps();
  rememberTranscriptScrollSnap("session-illustrated", {
    stickToBottom: false,
    scrollTop: 1440,
    anchor: { index: 7, key: "u-figure", offset: 36 },
  });
  assert.deepEqual(readTranscriptScrollSnap("session-illustrated"), {
    stickToBottom: false,
    scrollTop: 1440,
    anchor: { index: 7, key: "u-figure", offset: 36 },
  });
  resetTranscriptScrollSnaps();
});

check("wheel-up unsticks even inside the slack zone", () => {
  const stuck = { stickToBottom: true, released: false };
  const afterWheel = applyTranscriptStickOnWheelUp(stuck, -50);
  assert.equal(afterWheel.stickToBottom, false);
  assert.equal(afterWheel.released, true);
  // One mouse notch (~50px) is still inside 80px slack.
  const afterScroll = applyTranscriptStickOnScroll(afterWheel, 50, false);
  assert.equal(afterScroll.stickToBottom, false);
  assert.equal(afterScroll.released, true);
});

check("trackpad 1px up does not restick", () => {
  const afterWheel = applyTranscriptStickOnWheelUp(
    { stickToBottom: true, released: false },
    -1,
  );
  const afterScroll = applyTranscriptStickOnScroll(afterWheel, 1, false);
  assert.equal(afterScroll.stickToBottom, false);
  assert.equal(afterScroll.released, true);
  assert.ok(1 < TRANSCRIPT_RESTICK_PX);
});

check("leaving the slack zone re-arms stick for the next visit", () => {
  const released = { stickToBottom: false, released: true };
  const left = applyTranscriptStickOnScroll(released, TRANSCRIPT_NEAR_BOTTOM_PX, false);
  assert.equal(left.released, false);
  assert.equal(left.stickToBottom, false);
  const back = applyTranscriptStickOnScroll(left, 10, true);
  assert.equal(back.stickToBottom, true);
});

check("scroll down to the true end resticks without leaving the zone", () => {
  const released = { stickToBottom: false, released: true };
  const stillInZone = applyTranscriptStickOnScroll(released, 40, false);
  assert.equal(stillInZone.released, true);
  const atEnd = applyTranscriptStickOnScroll(stillInZone, 0, true);
  assert.equal(atEnd.stickToBottom, true);
  assert.equal(atEnd.released, false);
});

check("already stuck stays stuck through 40px layout jitter", () => {
  const stuck = { stickToBottom: true, released: false };
  const jitter = applyTranscriptStickOnScroll(stuck, 40, false);
  assert.equal(jitter.stickToBottom, true);
  assert.equal(jitter.released, false);
});

check("snap persists the live stick flag, not the 80px guess", () => {
  const el = { scrollTop: 870, scrollHeight: 1000, clientHeight: 100 };
  assert.equal(isNearTranscriptBottom(el), true);
  const snap = captureScrollSnap(el, false);
  assert.equal(snap.stickToBottom, false);
  assert.equal(snap.scrollTop, 870);
});

check("rows above the viewport compensate scrollTop when they grow", () => {
  assert.equal(compensateScrollForSizeChange(800, 100, 48, 300), 1052);
  assert.equal(compensateScrollForSizeChange(800, 780, 48, 300), 800);
  assert.equal(compensateScrollForSizeChange(0, 0, 48, 300), 0);
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
