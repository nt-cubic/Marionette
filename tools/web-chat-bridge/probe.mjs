#!/usr/bin/env node
//! NDJSON 探针：不起 Marionette，直接对桥跑一遍 ACP 全流程。
//!
//!   node probe.mjs echo                        — 本地自测站点（无外网、无登录，全流程可验）
//!   node probe.mjs echo-shared                 — 双开两座桥验证「共用一个浏览器实例」：
//!                                                同一浏览器、两个 tab、消息不串、收摊正确
//!   node probe.mjs chatgpt ["第一句" ["第二句"]] — 真站点冒烟（需已登录）
//!   node probe.mjs deepseek ["第一句" ["第二句"]]
//!
//! 固定跑两轮（同一会话）：第 2 轮断言「不重抄第 1 轮的回复」——这是
//! 2026-10-25 修的回归（网页会话流是全量历史，缺基线时每轮整段重发）。
//! echo 站点另外断言两轮的回显内容都正确。

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const site = process.argv[2] || "echo";
const text1 = process.argv[3] || "测试一下";
const text2 = process.argv[4] || "再来一句";

if (site === "echo-shared") {
  // 双开验证走 probe-shared.mjs；它自己 process.exit（含退出码），不能在这
  // 之后继续跑单桥逻辑，也不能提前 exit 把它刚装好的异步验证杀掉。
  await import("./probe-shared.mjs");
} else {
  runSingleBridge();
}

function runSingleBridge() {
  const proc = spawn(process.execPath, [path.join(here, "bridge.mjs"), site], {
    stdio: ["pipe", "pipe", "inherit"],
  });

  let buf = "";
  const saw = { init: false, new: false, result1: null, result2: null };
  let chunks = [];
  let turn1Text = "";
  const rev = (s) => s.split("").reverse().join("");

  proc.stdout.on("data", (d) => {
    buf += d.toString("utf8");
    let idx;
    while ((idx = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, idx).trim();
      buf = buf.slice(idx + 1);
      if (!line) continue;
      let msg;
      try { msg = JSON.parse(line); } catch { console.log("NON-JSON:", line.slice(0, 200)); continue; }
      handle(msg);
    }
  });

  function promptBlock(text) {
    // 首块故意带记忆卡 + 历史注入（桥必须剥掉，只发最后一段）。
    return [
      { type: "text", text: "<marionette-memory>\n## 测试记忆\n不该出现在网页输入框里\n</marionette-memory>" },
      { type: "image", data: "aGk=", mimeType: "image/png" },
      { type: "text", text: `[Marionette — prior conversation in this dialog]\nUser:\n旧消息\n\n[End of prior conversation. The user's new message follows.]\n${text}` },
    ];
  }

  function fail(msg) {
    console.error("PROBE FAILED:", msg);
    proc.kill();
    process.exit(1);
  }

  function handle(msg) {
    if (msg.id === 1) {
      saw.init = true;
      console.log("initialize ←", JSON.stringify(msg.result?.agentInfo || msg.result || msg.error));
      send({ jsonrpc: "2.0", id: 2, method: "session/new", params: { cwd: process.cwd(), mcpServers: [] } });
    } else if (msg.id === 2) {
      saw.new = true;
      saw.sessionId = msg.result.sessionId;
      console.log("session/new ←", JSON.stringify(msg.result));
      send({ jsonrpc: "2.0", id: 3, method: "session/prompt", params: { sessionId: msg.result.sessionId, prompt: promptBlock(text1) } });
    } else if (msg.id === 3) {
      saw.result1 = msg.result || msg.error;
      turn1Text = chunks.join("");
      console.log("turn 1 ←", JSON.stringify(saw.result1));
      console.log("--- turn 1 chunks ---");
      console.log(turn1Text || "(none)");
      console.log("---");
      if (!turn1Text.trim()) fail("turn 1 came back with no chunks");
      chunks = [];
      send({ jsonrpc: "2.0", id: 4, method: "session/prompt", params: { sessionId: saw.sessionId, prompt: [{ type: "text", text: text2 }] } });
    } else if (msg.id === 4) {
      saw.result2 = msg.result || msg.error;
      const turn2Text = chunks.join("");
      console.log("turn 2 ←", JSON.stringify(saw.result2));
      console.log("--- turn 2 chunks ---");
      console.log(turn2Text || "(none)");
      console.log("---");
      if (!turn2Text.trim()) fail("turn 2 came back with no chunks");
      if (site.startsWith("echo")) {
        if (!turn2Text.includes("PONG:" + rev(text2))) fail("turn 2 missing its own echo");
        if (turn2Text.includes("PONG:" + rev(text1))) fail("turn 2 re-sent turn 1's reply (baseline regression)");
      } else if (turn1Text.length >= 20 && turn2Text.includes(turn1Text)) {
        fail("turn 2 re-sent turn 1's reply (baseline regression)");
      }
      // 验完顺手发一个无 id 的 cancel 通知（桥不应崩溃）。
      send({ jsonrpc: "2.0", method: "session/cancel", params: { sessionId: "whatever" } });
      setTimeout(() => { proc.kill(); process.exit(0); }, 500);
    } else if (msg.method === "session/update") {
      const u = msg.params?.update || {};
      if (u.sessionUpdate === "agent_message_chunk" && u.content?.type === "text") {
        chunks.push(u.content.text);
      }
    }
  }

  function send(obj) { proc.stdin.write(JSON.stringify(obj) + "\n"); }

  proc.on("exit", (code) => {
    if (!saw.init || !saw.new || !saw.result1 || !saw.result2) {
      console.error(`PROBE FAILED (site=${site}) exit=${code} init=${saw.init} new=${saw.new} t1=${Boolean(saw.result1)} t2=${Boolean(saw.result2)}`);
      process.exit(1);
    }
  });

  setTimeout(() => {
    console.error(`PROBE TIMEOUT (site=${site}) init=${saw.init} new=${saw.new} t1=${Boolean(saw.result1)} t2=${Boolean(saw.result2)} chunks=${chunks.length}`);
    proc.kill();
    process.exit(1);
  }, site === "echo" ? 300_000 : 600_000);

  // 启动。
  send({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: 1, clientCapabilities: {} } });
}
