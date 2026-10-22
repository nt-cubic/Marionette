/**
 * Scenario buttons: user-definable one-click presets ("记录收藏", "新对话",
 * a Notion-style filing button, ...). Persisted via the Rust `scenario_*`
 * commands in ~/.marionette/scenarios.json (src-tauri/src/scenarios.rs);
 * these wrappers are the single frontend source of truth for the shape.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./api";

export type ScenarioTarget = "inbox" | "newChat";
export type ScenarioKind = "capture" | "prompt";

export type Scenario = {
  id: string;
  name: string;
  emoji: string;
  kind: ScenarioKind;
  /** Undefined = app default agent for the target. */
  agentId?: string;
  /** Optional per-scenario chips; applied like Telegram /new wizard prefs. */
  modelId?: string;
  modeId?: string;
  /** `capture` scenarios: template with a {content} placeholder. */
  promptTemplate?: string;
  target: ScenarioTarget;
  builtin?: boolean;
};

/**
 * Default organizing prompt for the built-in capture scenario. The agent
 * maintains the markdown knowledge base under the inbox project cwd
 * (~/.marionette/inbox): kb/<topic>.md entries, todo.md routing, README.md
 * index. See docs/plan-orb-assistant.md §2.
 */
export const DEFAULT_CAPTURE_TEMPLATE = `新收藏进来了，请整理进知识库：

{content}

要求：
1. 判断主题，归并进 kb/<主题>.md 的对应小节；没有合适的文件就新建（文件名用简短英文 slug）；
2. 每条记录包含：一句话摘要、#标签、收录日期、出处链接（如有）；
3. 如果内容是待办/提醒性质（「记得…」「要…」「下周…」之类），同时在 todo.md 登记一条；
4. 有新文件或新小节时更新 README.md 的主题索引；
5. 与已有条目去重合并，不要重复收录；除整理归档外不要做其他事。`;

export const BUILTIN_SCENARIOS: Scenario[] = [
  {
    id: "builtin-capture",
    name: "记录收藏",
    emoji: "📥",
    kind: "capture",
    promptTemplate: DEFAULT_CAPTURE_TEMPLATE,
    target: "inbox",
    builtin: true,
  },
  {
    id: "builtin-new-chat",
    name: "新对话",
    emoji: "✨",
    kind: "prompt",
    promptTemplate: "",
    target: "newChat",
    builtin: true,
  },
];

/** Merge stored scenarios over the builtins (builtins always present, in order). */
export function mergeBuiltinScenarios(stored: Scenario[]): Scenario[] {
  const custom = stored.filter((s) => !s?.id?.startsWith("builtin-"));
  return [...BUILTIN_SCENARIOS, ...custom];
}

export async function listScenarios(): Promise<Scenario[]> {
  if (!isTauriRuntime()) return BUILTIN_SCENARIOS;
  try {
    const stored = await invoke<Scenario[]>("scenario_list");
    return mergeBuiltinScenarios(Array.isArray(stored) ? stored : []);
  } catch {
    return BUILTIN_SCENARIOS;
  }
}

/** Replaces the whole list; Rust keeps the file, builtins are re-merged on load. */
export async function saveScenarios(scenarios: Scenario[]): Promise<Scenario[]> {
  if (!isTauriRuntime()) return mergeBuiltinScenarios(scenarios);
  const stored = await invoke<Scenario[]>("scenario_save", { scenarios });
  return mergeBuiltinScenarios(Array.isArray(stored) ? stored : []);
}

/**
 * Render a capture/prompt template: replaces {content} with the formatted
 * payload text. Used by the main window when executing a scenario.
 */
export function renderScenarioTemplate(
  template: string | undefined,
  content: string
): string {
  const body = (template ?? "").trim().length
    ? template!
    : DEFAULT_CAPTURE_TEMPLATE;
  // tsconfig lib is ES2020 — no String.replaceAll.
  return body.replace(/\{content\}/g, content);
}
