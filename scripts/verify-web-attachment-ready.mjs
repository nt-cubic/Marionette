import assert from "node:assert/strict";
import { uploadAttachments } from "../tools/web-chat-bridge/attachments.mjs";

const files = [{ path: "C:/test/original.jpg", name: "original.jpg" }];
const base = { inputs: [{ accept: "", multiple: true }], seen: [false], attachmentCount: 0, enabled: true, busy: false, errors: [] };
let inspections = 0;
let waitedForChange = false;
let disconnected = false;
const cdp = { send: async method => method === "DOM.getDocument" ? { root: { nodeId: 1 } } : method === "DOM.querySelector" ? { nodeId: 2 } : {} };
const evaluate = async (_page, expression) => {
  if (expression.includes("state.listeners.add(done)")) waitedForChange = true;
  if (expression.includes("state.observer.disconnect()")) disconnected = true;
  if (expression.includes("function inspectComposer")) {
    inspections++;
    if (inspections === 1) return base;
    if (inspections === 2) return { ...base, attachmentCount: 1, busy: true };
    return { ...base, attachmentCount: 1 }; // Preview renamed; original name is absent.
  }
  if (expression.includes("window.__marionettePendingUpload ||")) return [];
  if (expression.includes("data-marionette-upload")) return { ok: true };
  return {};
};
assert.equal(await uploadAttachments({ cdp, sessionId: "test" }, "chatgpt", files, evaluate, () => false), true);
assert.equal(inspections, 3, "wait for upload to stop, even when the preview already exists");
assert.ok(waitedForChange, "wait for DOM changes while the upload is busy");
assert.ok(disconnected, "disconnect the observer after upload");
await assert.rejects(uploadAttachments({ cdp }, "chatgpt", files, async () => ({ ...base, attachmentCount: 1 }), () => false), /已有附件/);
console.log("Web attachment readiness checks passed");
