/**
 * Tool-call image extraction + preview src collection.
 * Run: npx tsx scripts/smoke-tool-images.mts
 */
import assert from "node:assert/strict";
import {
  applyAcpPartToEvents,
  extractAcpUpdateText,
} from "../src/lib/acpTranscript.ts";
import {
  collectToolImageSrcs,
  isImagePath,
  stripFileUri,
} from "../src/lib/imageAttachments.ts";
import { imageLinkTarget } from "../src/lib/toolBody.ts";
import { classifyToolCall, toolMainText, type ToolCallEvent } from "../src/lib/turnActivity.ts";

assert.equal(stripFileUri("file:///D:/shots/a.png"), "D:/shots/a.png");
assert.equal(stripFileUri("file://localhost/C:/x/y.jpg"), "C:/x/y.jpg");
assert.equal(stripFileUri("D:\\shots\\a.png"), "D:\\shots\\a.png");
assert.equal(isImagePath("D:\\shots\\a.png"), true);
assert.equal(isImagePath("file:///D:/shots/a.webp"), true);
assert.equal(isImagePath("readme.md"), false);

assert.equal(imageLinkTarget("D:/shots/a.png"), "D:/shots/a.png");
assert.equal(imageLinkTarget("file:///D:/shots/a.png"), "D:/shots/a.png");
assert.equal(imageLinkTarget("D:/shots/a.png?raw=1#top"), "D:/shots/a.png");
assert.equal(imageLinkTarget("https://cdn.example/shot.jpg?w=800"), "https://cdn.example/shot.jpg?w=800");
assert.equal(imageLinkTarget("https://example.com/docs/page"), null);
assert.equal(imageLinkTarget("D:/notes/readme.md"), null);
assert.equal(imageLinkTarget("#section"), null);

const viewImage = {
  type: "tool_call",
  toolName: "view image",
  title: "view image",
  path: "D:\\shots\\hero.png",
} as unknown as ToolCallEvent;
assert.equal(classifyToolCall(viewImage), "read");
assert.equal(toolMainText(viewImage), "hero.png");

const fromPath = collectToolImageSrcs({ path: "D:\\shots\\hero.png" });
assert.deepEqual(fromPath, ["D:\\shots\\hero.png"]);

const fromInput = collectToolImageSrcs({
  input: JSON.stringify({ path: "C:/tmp/x.jpeg" }),
});
assert.deepEqual(fromInput, ["C:/tmp/x.jpeg"]);

const fromImages = collectToolImageSrcs({
  images: ["file:///D:/a.png", "https://example.com/b.jpg"],
});
assert.deepEqual(fromImages, ["D:/a.png", "https://example.com/b.jpg"]);

assert.deepEqual(
  collectToolImageSrcs({ input: JSON.stringify({ command: "ls foo.png" }) }),
  [],
  "shell command must not be treated as an image path",
);
assert.deepEqual(
  collectToolImageSrcs({ input: "ls foo.png" }),
  [],
  "plain command string must not be treated as an image path",
);

const located = extractAcpUpdateText({
  sessionUpdate: "tool_call",
  toolCallId: "t-read-1",
  title: "Read",
  kind: "read",
  status: "completed",
  locations: [{ path: "D:\\shots\\a.png" }],
  content: [
    {
      type: "content",
      content: {
        type: "image",
        mimeType: "image/png",
        uri: "file:///D:/shots/a.png",
        data: "iVBORw0KGgo=",
      },
    },
  ],
});
assert.ok(located);
assert.equal(located.role, "tool");
assert.equal(located.toolPath, "D:\\shots\\a.png");
assert.deepEqual(located.toolImages, ["D:/shots/a.png"]);
assert.match(located.toolDetail ?? "", /\[image\]/);

const fromRawInput = extractAcpUpdateText({
  sessionUpdate: "tool_call",
  toolCallId: "t-read-2",
  title: "read_file",
  status: "completed",
  rawInput: { path: "C:\\Users\\me\\pic.webp" },
});
assert.ok(fromRawInput);
assert.equal(fromRawInput.toolPath, "C:\\Users\\me\\pic.webp");

const uriOnly = extractAcpUpdateText({
  sessionUpdate: "tool_call_update",
  toolCallId: "t-read-3",
  content: [
    {
      type: "image",
      mimeType: "image/jpeg",
      uri: "https://cdn.example/shot.jpg",
    },
  ],
});
assert.ok(uriOnly);
assert.equal(uriOnly.toolPath, "https://cdn.example/shot.jpg");
assert.deepEqual(uriOnly.toolImages, ["https://cdn.example/shot.jpg"]);

let events = applyAcpPartToEvents([], "s1", located);
const card = events.find((e) => e.type === "tool_call");
assert.ok(card && card.type === "tool_call");
assert.deepEqual(card.images, ["D:/shots/a.png"]);
assert.equal(card.path, "D:\\shots\\a.png");

events = applyAcpPartToEvents(events, "s1", {
  role: "tool",
  text: "Read · completed",
  isDelta: false,
  sessionUpdate: "tool_call_update",
  toolCallId: "t-read-1",
  toolStatus: "completed",
});
const updated = events.find((e) => e.type === "tool_call");
assert.ok(updated && updated.type === "tool_call");
assert.deepEqual(updated.images, ["D:/shots/a.png"], "status ping must keep images");

console.log("smoke-tool-images: ok");
