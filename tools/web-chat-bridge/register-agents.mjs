#!/usr/bin/env node
//! 注册「ChatGPT 网页版 / DeepSeek 网页版」两个自定义 agent 到 Marionette。
//!
//! 为什么是 node + 绝对路径参数，而不是一个 .cmd 垫片命令：process_util 对
//! .cmd 会做「拆 node」优化，会把非 node_modules 形态的垫片拆成裸 node
//! （参数全丢 → node 进 REPL 假死）。直接注册 node + args 绕开整个垫片机制。
//! 幂等：已存在（按 id）则更新命令与参数。
//!
//! 用法：node register-agents.mjs

import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const bridge = path.join(here, "bridge.mjs");
if (!fs.existsSync(bridge)) {
  console.error(`bridge.mjs not found: ${bridge}`);
  process.exit(1);
}

const target = path.join(os.homedir(), ".marionette", "custom_agents.json");
let list = [];
try {
  const raw = fs.readFileSync(target, "utf8");
  if (raw.trim()) list = JSON.parse(raw);
  if (!Array.isArray(list)) list = [list];
} catch (err) {
  if (err.code !== "ENOENT") {
    console.error(`existing ${target} is unreadable (${err.message}) — fix or remove it first`);
    process.exit(1);
  }
}

const agents = [
  {
    id: "web-chatgpt",
    site: "chatgpt",
    label: "ChatGPT 网页版",
    note: "纯聊天：桥接 ChatGPT 网页版，额度走网页份额。首次使用会弹出一个浏览器窗口，请在那里登录一次。",
  },
  {
    id: "web-deepseek",
    site: "deepseek",
    label: "DeepSeek 网页版",
    note: "纯聊天：桥接 DeepSeek 网页版，额度走网页份额。首次使用会弹出一个浏览器窗口，请在那里登录一次。",
  },
];

for (const a of agents) {
  const entry = {
    id: a.id,
    label: a.label,
    command: process.execPath,
    args: [bridge, a.site],
    npmPackage: null,
    note: a.note,
  };
  const existing = list.find((x) => x && x.id === a.id);
  if (existing) {
    existing.command = entry.command;
    existing.args = entry.args;
    existing.label = entry.label;
    existing.note = entry.note;
    console.log(`updated ${a.id} (command=${entry.command})`);
  } else {
    list.push(entry);
    console.log(`added ${a.id} (command=${entry.command})`);
  }
}

fs.mkdirSync(path.dirname(target), { recursive: true });
fs.writeFileSync(target, JSON.stringify(list, null, 2) + "\n", "utf8");
console.log(`written ${target}`);
