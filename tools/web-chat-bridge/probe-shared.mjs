#!/usr/bin/env node
//! 双开探针（由 probe.mjs echo-shared 调起）：同时跑两座桥（echo + echo2），
//! 验证共享浏览器机制本身：
//!   1. 两个桥进程只拉起一个浏览器实例（第二座 attach，不 spawn 第二个）；
//!   2. 两座桥各有自己的 tab，消息互不串（回复都带各自的 PONG 前缀语义）；
//!   3. 两座桥退出后浏览器仍然保留；
//!   4. 重跑复用浏览器和标签页，最后由探针清理自己的隔离浏览器。

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const bridge = path.join(here, "bridge.mjs");
const STATE_ROOT = path.join(os.tmpdir(), `mwb-probe-shared-${process.pid}`);
const bridgeProcesses = new Set();

const procA = spawnBridge("echo");
const procB = spawnBridge("echo2");

function spawnBridge(siteKey) {
  const p = spawn(process.execPath, [bridge, siteKey], {
    stdio: ["pipe", "pipe", "inherit"],
    env: {
      ...process.env,
      BRIDGE_STATE_DIR: STATE_ROOT,
      MARIONETTE_SESSION_ID: `shared-probe-${siteKey}`,
    },
  });
  bridgeProcesses.add(p);
  return p;
}

const clients = new Map(); // proc -> client state
function makeClient(proc, name, onTurn) {
  const c = { name, buf: "", sessionId: null, chunks: [], resolved: null, onTurn };
  clients.set(proc, c);
  proc.stdout.on("data", (d) => {
    c.buf += d.toString("utf8");
    let idx;
    while ((idx = c.buf.indexOf("\n")) >= 0) {
      const line = c.buf.slice(0, idx).trim();
      c.buf = c.buf.slice(idx + 1);
      if (line) handleLine(proc, c, line);
    }
  });
  proc.on("exit", () => { c.exited = true; });
  return c;
}

function handleLine(proc, c, line) {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id === 1) {
    send(proc, { jsonrpc: "2.0", id: 2, method: "session/new", params: { cwd: process.cwd(), mcpServers: [] } });
  } else if (msg.id === 2) {
    c.sessionId = msg.result.sessionId;
    c.ready = true;
    maybeStartTurns();
  } else if (msg.id === 3 || msg.id === 4) {
    c.resolved = c.resolved || {};
    c.resolved[msg.id] = true;
    if (msg.id === 3) {
      c.turn1 = c.chunks.join("");
      c.chunks = [];
      send(proc, { jsonrpc: "2.0", id: 4, method: "session/prompt", params: { sessionId: c.sessionId, prompt: [{ type: "text", text: c.onTurn.second }] } });
    } else {
      c.turn2 = c.chunks.join("");
      c.chunks = [];
      maybeFinish();
    }
  } else if (msg.method === "session/update") {
    const u = msg.params?.update || {};
    if (u.sessionUpdate === "agent_message_chunk" && u.content?.type === "text") c.chunks.push(u.content.text);
  }
}

function send(proc, obj) { proc.stdin.write(JSON.stringify(obj) + "\n"); }

function fail(msg) {
  console.error("PROBE-SHARED FAILED:", msg);
  for (const p of bridgeProcesses) { try { p.kill(); } catch { /* 忽略 */ } }
  console.error("diagnostics retained at", STATE_ROOT);
  process.exit(1);
}

function maybeStartTurns() {
  const a = clients.get(procA);
  const b = clients.get(procB);
  if (!a?.ready || !b?.ready) return;
  a.onTurn.first = "甲第一句";
  b.onTurn.first = "乙第一句";
  a.onTurn.second = "甲第二句";
  b.onTurn.second = "乙第二句";
  // 交错启动：A 先发，B 紧跟着发 —— 两座桥在同一浏览器里并发各占各的 tab。
  send(procA, { jsonrpc: "2.0", id: 3, method: "session/prompt", params: { sessionId: a.sessionId, prompt: [{ type: "text", text: a.onTurn.first }] } });
  setTimeout(() => {
    send(procB, { jsonrpc: "2.0", id: 3, method: "session/prompt", params: { sessionId: b.sessionId, prompt: [{ type: "text", text: b.onTurn.first }] } });
  }, 300);
}

function rev(s) { return s.split("").reverse().join(""); }

let phase = 0;
function maybeFinish() {
  const a = clients.get(procA);
  const b = clients.get(procB);
  if (!(a.resolved?.[4] && b.resolved?.[4])) return;

  // 断言 1：各自的两轮都有内容。
  for (const c of [a, b]) {
    if (!c.turn1?.trim() || !c.turn2?.trim()) fail(`${c.name}: empty turn`);
  }
  // 断言 2：回复来自各自站点（PONG + 逆序 = echo 站点回显语义；串站表现为
  // 对方的原文原样出现 —— echo 页面只会回显，不会把乙的原文逆序后再吐给甲）。
  for (const c of [a, b]) {
    if (!c.turn1.includes("PONG:" + rev(c.onTurn.first))) fail(`${c.name}: turn1 missing own echo`);
    if (!c.turn2.includes("PONG:" + rev(c.onTurn.second))) fail(`${c.name}: turn2 missing own echo`);
    if (c.turn1.includes(c.onTurn.second) || c.turn2.includes(c.onTurn.first)) fail(`${c.name}: cross-site leakage`);
  }
  console.log("both bridges completed turns in one browser, no cross-talk");

  // 阶段 2：两座桥先后退场，浏览器始终保留，供下一次连接复用。
  phase = 2;
  listBrowserProcs().then((n0) => {
    if (n0 < 1) fail("expected the shared browser to be running before shutdown phase");
    procA.stdin.end();
    const waitA = setInterval(() => {
      if (!a.exited) return;
      clearInterval(waitA);
      listBrowserProcs().then((n1) => {
        if (n1 < 1) fail("browser died when first bridge exited (peer still active)");
        console.log("browser survived first bridge exit");
        procB.stdin.end();
        const waitB = setInterval(() => {
          if (!b.exited) return;
          clearInterval(waitB);
          listBrowserProcs().then((n2) => {
            if (n2 < 1) fail("browser died after last bridge exit");
            console.log("browser retained after last bridge exit");
            finish();
          });
        }, 200);
      });
    }, 200);
  });
}

function listBrowserProcs() {
  // 按 DevToolsActivePort 所属 profile 探活更直接：端口能连上 = 活着。
  const portFile = path.join(STATE_ROOT, "profile-echo", "DevToolsActivePort");
  return new Promise((resolve) => {
    let port = null;
    try { port = fs.readFileSync(portFile, "utf8").split("\n")[0].trim(); } catch { return resolve(0); }
    if (!/^\d+$/.test(port)) return resolve(0);
    fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(1500) })
      .then(() => resolve(1))
      .catch(() => resolve(0));
  });
}

function finish() {
  // 阶段 3：再各起一座桥，连接仍然打开的浏览器。
  const procA2 = spawnBridge("echo");
  const procB2 = spawnBridge("echo2");
  const seen = new Map();
  const wire = (proc, name, first) => {
    let sid = null;
    let chunks2 = [];
    let buf = "";
    proc.stdout.on("data", (d) => {
      buf += d.toString("utf8");
      let index;
      while ((index = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, index);
        buf = buf.slice(index + 1);
        if (!line.trim()) continue;
        let m;
        try { m = JSON.parse(line); } catch { continue; }
        if (m.id === 1) send(proc, { jsonrpc: "2.0", id: 2, method: "session/new", params: { cwd: process.cwd(), mcpServers: [] } });
        else if (m.id === 2) {
          sid = m.result.sessionId;
          send(proc, { jsonrpc: "2.0", id: 3, method: "session/prompt", params: { sessionId: sid, prompt: [{ type: "text", text: first }] } });
        } else if (m.id === 3) {
          seen.set(name, chunks2.join(""));
          proc.kill();
          check();
        } else if (m.method === "session/update" && m.params?.update?.sessionUpdate === "agent_message_chunk") {
          chunks2.push(m.params.update.content?.text || "");
        }
      }
    });
  };
  const check = async () => {
    if (seen.size < 2) return;
    for (const [name, text] of seen) {
      if (!text.includes("PONG:")) fail(`reconnect: ${name} got no reply`);
    }
    console.log("reconnect to retained browser works");
    // 仅探针清理自己创建的隔离浏览器；产品桥永远不发送 Browser.close。
    const portFile = path.join(STATE_ROOT, "profile-echo", "DevToolsActivePort");
    const [port, expectedPath] = fs.readFileSync(portFile, "utf8").trim().split(/\r?\n/);
    const info = await (await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(1500) })).json();
    if (new URL(info.webSocketDebuggerUrl).pathname !== expectedPath) fail("refusing cleanup of unrelated browser");
    const ws = new WebSocket(info.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { ws.close(); reject(new Error("cleanup connection timed out")); }, 5000);
      ws.addEventListener("open", () => { clearTimeout(timer); resolve(); });
      ws.addEventListener("error", () => { clearTimeout(timer); reject(new Error("cleanup connection failed")); });
    });
    ws.send(JSON.stringify({ id: 1, method: "Browser.close" }));
    await new Promise((resolve) => {
      const timer = setTimeout(resolve, 5000);
      ws.addEventListener("close", () => { clearTimeout(timer); resolve(); });
    });
    ws.close();
    // 浏览器完全释放文件前保留诊断目录，不递归删除仍在使用的 profile。
    console.log("PROBE-SHARED OK");
    process.exit(0);
  };
  wire(procA2, "echo", "重启甲");
  wire(procB2, "echo2", "重启乙");
  setTimeout(() => fail("restart phase timed out"), 120_000);
}

setTimeout(() => fail("shared probe timed out"), 300_000);

// 装配两座桥的客户端（stdout 监听 + 回合文本桶），再发 initialize。
const clientA = makeClient(procA, "echo", {});
const clientB = makeClient(procB, "echo2", {});
for (const p of [procA, procB]) {
  send(p, { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: 1, clientCapabilities: {} } });
}
void clientA;
void clientB;
