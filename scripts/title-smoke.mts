/**
 * Session-title smoke test.
 *
 * Run: npx tsx scripts/title-smoke.mts
 *
 * Both preambles below are the real strings Marionette injects
 * (`sessionHistory.ts` and `context_inventory.rs`). An agent titles a
 * conversation from its first prompt, so after a reconnect it hands one of
 * these back — which is how a whole shelf of dialogs became "[Marionette — …".
 */
import assert from "node:assert/strict";
import {
  isInjectedPromptText,
  parseTranscriptEvents,
  titleFromUserText,
} from "../src/lib/transcript.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const HISTORY_PREAMBLE = [
  "[Marionette — prior conversation in this dialog]",
  "The UI has history from earlier in this thread. Your ACP session was freshly started",
  "(e.g. app restart), so you do NOT have this context yet. Treat the following as",
  "what already happened. Continue naturally; do not claim the history is empty.",
  "",
  "User: 帮我把用量面板修一下",
].join("\n");

const SKILLS_PREAMBLE =
  "[Marionette — skills available in this project]\n" +
  "These are instruction files on this machine. If one matches the task, read its " +
  "SKILL.md first and follow it. Ignore the rest.\n\n- **agent-reach**";

console.log("injected preambles are refused as titles");
check("both headers are recognised", () => {
  assert.equal(isInjectedPromptText(HISTORY_PREAMBLE), true);
  assert.equal(isInjectedPromptText(SKILLS_PREAMBLE), true);
});
check("the agent's truncated copy is recognised too", () => {
  assert.equal(isInjectedPromptText("[Marionette — prior conversation in this dialo…"), true);
  assert.equal(isInjectedPromptText("[Marionette — skills available in this pro…"), true);
});
check("ordinary brackets and prose are not mistaken for it", () => {
  assert.equal(isInjectedPromptText("[urgent] fix the usage bar"), false);
  assert.equal(isInjectedPromptText("Marionette — a normal message"), false);
  assert.equal(isInjectedPromptText("我想看看 [Marionette] 这个名字"), false);
});

console.log("\ntitles come from what the user typed");
check("a short message is its own title", () => {
  assert.equal(titleFromUserText("修一下用量面板"), "修一下用量面板");
});
check("a long message is cut to 46 characters plus an ellipsis", () => {
  const long =
    "帮我把用量面板的 Grok 数字核对一下，另外看看 Claude 和 Codex 的额度是不是也有同样的问题";
  const title = titleFromUserText(long);
  assert.equal([...title].length, 47);
  assert.equal(title.endsWith("…"), true);
});
check("whitespace collapses onto one line", () => {
  assert.equal(titleFromUserText("第一行\n\n第二行"), "第一行 第二行");
});
check("empty text still falls back to the placeholder", () => {
  assert.equal(titleFromUserText("   \n  "), "New session");
});

console.log("\nrepair reads the first typed message out of the transcript");
check("the first typed message becomes the label", () => {
  const events = parseTranscriptEvents([
    { type: "assistant_message", sessionId: "s1", text: "好的", createdAt: "2026-10-08T10:00:00Z" },
    { type: "user_message", sessionId: "s1", text: "把用量面板核对一下", createdAt: "2026-10-08T10:00:01Z" },
  ]);
  const first = events.find(
    (event) => event.type === "user_message" && !isInjectedPromptText(event.text),
  );
  assert.ok(first && first.type === "user_message");
  assert.equal(titleFromUserText(first.text), "把用量面板核对一下");
});
check("a transcript holding only the preamble yields no label", () => {
  const events = parseTranscriptEvents([
    { type: "user_message", sessionId: "s1", text: HISTORY_PREAMBLE, createdAt: "2026-10-08T10:00:00Z" },
  ]);
  const first = events.find(
    (event) => event.type === "user_message" && !isInjectedPromptText(event.text),
  );
  assert.equal(first, undefined);
});

console.log(`\n${passed} checks passed.`);
