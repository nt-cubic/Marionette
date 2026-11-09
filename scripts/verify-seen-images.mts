// Regression for the seen-images feature data layer:
// src/lib/seenImages.ts (collect / group / filter / locate) and src/lib/imageSupport.ts (three-state).
// Run: npx tsx scripts/verify-seen-images.mts
import {
  collectSeenImages,
  countSeenBySource,
  estimateSeenTokens,
  filterSeen,
  findSeenEventIndex,
  formatClock,
  groupSeenByAge,
  toolVia,
  type SeenImage,
} from "../src/lib/seenImages";
import { bridgeCatalogId, resolveImageSupport } from "../src/lib/imageSupport";
import type { SessionEvent } from "../src/lib/types";

let failed = 0;
function check(name: string, ok: boolean, detail?: unknown) {
  if (ok) console.log("  ok ", name);
  else {
    failed += 1;
    console.log("  FAIL", name, detail ?? "");
  }
}

const sid = "s1";
const at = (iso: string) => new Date(iso).toISOString();
const events: SessionEvent[] = [
  { type: "user_message", sessionId: sid, text: "see these", createdAt: at("2026-11-08T10:00:00"), messageId: "u1", attachments: [{ id: "a", path: "D:\\shots\\ui.png", name: "ui.png", mimeType: "image/png", marks: [] }] },
  { type: "tool_call", sessionId: sid, text: "", toolCallId: "t1", toolName: "browser_take_screenshot", createdAt: at("2026-11-08T10:01:00"), completedAt: at("2026-11-08T10:01:02"), path: "D:/shots/page.png" },
  { type: "tool_call", sessionId: sid, text: "", toolCallId: "t2", toolName: "read", createdAt: at("2026-11-08T10:02:00"), path: "shot.png" },
  // the same picture again, later, by a different tool → one entry, latest wins
  { type: "tool_call", sessionId: sid, text: "", toolCallId: "t3", toolName: "read", createdAt: at("2026-11-08T10:05:00"), path: "d:/shots/PAGE.png" },
  { type: "tool_call", sessionId: sid, text: "", toolCallId: "t4", toolName: "bash", createdAt: at("2026-11-08T10:06:00"), input: "ls shot.png" },
  { type: "tool_call", sessionId: sid, text: "", toolCallId: "t5", toolName: "fetch", createdAt: at("2026-11-08T10:07:00"), images: ["https://example.com/a.png", "data:image/png;base64,AAAA"] },
  { type: "assistant_message", sessionId: sid, text: "done", createdAt: at("2026-11-08T10:08:00") },
];

const seen = collectSeenImages(events, "D:/proj");
check("one entry per distinct picture", seen.length === 4, seen.map((s) => s.src));
const page = seen.find((s) => s.key.endsWith("shots/page.png"));
check("same picture twice: uses counted, latest card wins", page?.uses === 2 && page.ref.type === "tool_call" && page.ref.toolCallId === "t3", page);
check("later sighting by another tool changes via", page?.via === "read");
const ui = seen.find((s) => s.name === "ui.png");
check("user attachment → via you, card is the user message", ui?.via === "you" && ui.ref.type === "user_message" && ui.ref.messageId === "u1", ui);
const rel = seen.find((s) => s.name === "shot.png");
check("relative tool path resolves against cwd", rel?.src === "D:/proj/shot.png", rel?.src);
check("shell command text is not an image", !seen.some((s) => s.src.includes("ls ")));
check("remote url kept, data url dropped", seen.some((s) => s.src === "https://example.com/a.png") && !seen.some((s) => s.src.startsWith("data:")));
check("newest first", seen.every((s, i) => i === 0 || seen[i - 1]!.at >= s.at));
check("tool via classification", toolVia("browser_take_screenshot") === "browser" && toolVia("mcp__computer__screenshot") === "computer" && toolVia("read") === "read" && toolVia(undefined, "Navigate") === "browser");

// counting / filtering / tokens
const counts = countSeenBySource(seen);
check("counts split you / tool", counts.all === 4 && counts.you === 1 && counts.tool === 3, counts);
check("filter you / tool / all", filterSeen(seen, "you").length === 1 && filterSeen(seen, "tool").length === 3 && filterSeen(seen, "all").length === 4);
check("token estimate is flat per image", estimateSeenTokens(53) === 53_000);

// grouping (viewer's local clock)
const now = new Date(2026, 10, 9, 12, 0, 0);
const mk = (d: Date): SeenImage => ({ key: String(d.getTime()), src: String(d.getTime()), name: "x", via: "read", at: d.getTime(), uses: 1, ref: { type: "tool_call", createdAt: "" } });
const imgs = [
  mk(new Date(2026, 10, 9, 11, 55)), // 5 min ago → now
  mk(new Date(2026, 10, 9, 8, 0)), // today
  mk(new Date(2026, 10, 8, 20, 0)), // yesterday
  mk(new Date(2026, 10, 8, 9, 0)), // yesterday
  mk(new Date(2026, 10, 3, 9, 0)), // 11/3
  mk(new Date(2026, 10, 3, 8, 0)), // 11/3
  mk(new Date(2026, 9, 30, 8, 0)), // 10/30
];
const groups = groupSeenByAge(imgs, now);
check("groups: now / today / yesterday / per older day", groups.map((g) => g.kind).join() === "now,today,yesterday,day,day", groups.map((g) => g.key));
check("groups keep order and sizes", groups.map((g) => g.items.length).join() === "1,1,2,2,1");
check("older day carries month/day", groups[3]?.month === 11 && groups[3]?.day === 3 && groups[4]?.month === 10 && groups[4]?.day === 30);
check("clock is HH:MM", formatClock(new Date(2026, 10, 9, 6, 8).getTime()) === "06:08");
check("grouping an empty list", groupSeenByAge([], now).length === 0);

// locate
check("locate tool card by toolCallId", findSeenEventIndex(events, { type: "tool_call", toolCallId: "t3", createdAt: "" }) === 3);
check("locate user card by messageId", findSeenEventIndex(events, { type: "user_message", messageId: "u1", createdAt: "" }) === 0);
check("locate falls back to createdAt", findSeenEventIndex(events, { type: "tool_call", createdAt: events[2]!.createdAt }) === 2);
check("locate misses cleanly", findSeenEventIndex(events, { type: "tool_call", toolCallId: "nope", createdAt: "x" }) === -1);

// capability: three-state, reported not guessed
check("bridge id forms", bridgeCatalogId("marionette/opencode-go/deepseek-v4.1-flash") === "opencode-go/deepseek-v4.1-flash" && bridgeCatalogId("marionette::p/m") === "p/m" && bridgeCatalogId("marionette\\p/m") === "p/m" && bridgeCatalogId("gpt-6") === null);
check("nothing reported → unknown", resolveImageSupport({ modelId: "deepseek-v4.1" }) === "unknown");
check("agent refuses images → no (even if model says yes)", resolveImageSupport({ agentPromptImage: false, modelId: "m", modelImages: { m: true } }) === "no");
check("agent accepts, model silent → unknown (never assume)", resolveImageSupport({ agentPromptImage: true, modelId: "m" }) === "unknown");
check("agent's own model catalog says yes / no", resolveImageSupport({ modelId: "m", modelImages: { m: true } }) === "yes" && resolveImageSupport({ modelId: "m", modelImages: { m: false } }) === "no");
check("model id lookup ignores case", resolveImageSupport({ modelId: "Grok-4.6", modelImages: { "grok-4.6": true } }) === "yes");
const engine = { "opencode-go/deepseek-v4.1-flash": true, "opencode-go/deepseek-v4-pro": false };
check("bridged model → engine catalog", resolveImageSupport({ modelId: "marionette/opencode-go/deepseek-v4.1-flash", engineImages: engine }) === "yes" && resolveImageSupport({ modelId: "marionette::opencode-go/deepseek-v4-pro", engineImages: engine }) === "no");
check("bridged model missing from catalog → unknown", resolveImageSupport({ modelId: "marionette/ghost/model", engineImages: engine }) === "unknown");
check("no model-name guessing: a vision-sounding name alone means nothing", resolveImageSupport({ modelId: "deepseek-v4-vision" }) === "unknown" && resolveImageSupport({ modelId: "text-only-model" }) === "unknown");

console.log(failed === 0 ? "\n全部通过" : `\n${failed} 项失败`);
process.exit(failed === 0 ? 0 : 1);
