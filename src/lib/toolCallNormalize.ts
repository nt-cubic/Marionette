/**
 * Tool name / kind normalization (Codeg-aligned, pure data — no UI).
 * Stabilizes Grok title drift, Codex collab names, Cline aliases, etc.
 */

const EXACT_ALIASES: Record<string, string> = {
  shell_command: "bash",
  "functions.shell_command": "bash",
  run_terminal_command: "bash",
  shell: "bash",
  exec_command: "bash",
  write_stdin: "bash",
  execute_command: "bash",
  read_file: "read",
  read_text_file: "read",
  readfile: "read",
  "read file": "read",
  "functions.read": "read",
  edit_file: "edit",
  update_file: "edit",
  write_file: "write",
  "functions.edit": "edit",
  "functions.write": "write",
  "functions.apply_patch": "apply_patch",
  change: "edit",
  changes: "edit",
  write_to_file: "write",
  replace_in_file: "edit",
  writefile: "write",
  editfile: "edit",
  searchtext: "grep",
  search_text: "grep",
  search_files: "grep",
  list_files: "glob",
  list_code_definition_names: "grep",
  todowrite: "todowrite",
  todo_write: "todowrite",
  enter_plan_mode: "enter_plan_mode",
  exit_plan_mode: "exit_plan_mode",
  web_fetch: "web_fetch",
  web_search: "web_search",
  browser_action: "web_fetch",
  attempt_completion: "attempt_completion",
  ask_followup_question: "question",
  request_user_input: "question",
  spawn_agent: "agent",
  wait_agent: "task",
  close_agent: "task",
  update_plan: "task",
  use_mcp_tool: "tool",
  // Codex structured goal (session_info_update → synthetic tool)
  create_goal: "create_goal",
  update_goal: "update_goal",
  goal: "create_goal",
};

function stripNoise(name: string): string {
  return name
    .trim()
    .replace(/^functions\./i, "")
    .replace(/^mcp__[^_]+__/i, "mcp__")
    .replace(/\s+/g, " ");
}

/** Canonical tool key for classification (lowercase, aliases applied). */
export function normalizeToolName(raw: string | null | undefined): string {
  if (!raw) return "";
  const stripped = stripNoise(raw);
  const key = stripped.toLowerCase().replace(/-/g, "_");
  if (EXACT_ALIASES[key]) return EXACT_ALIASES[key];
  if (EXACT_ALIASES[stripped.toLowerCase()]) return EXACT_ALIASES[stripped.toLowerCase()];
  // Compact form without underscores for table lookup
  const compact = key.replace(/_/g, "");
  for (const [alias, canon] of Object.entries(EXACT_ALIASES)) {
    if (alias.replace(/_/g, "") === compact) return canon;
  }
  return key || stripped.toLowerCase();
}

/**
 * Grok stamps identity in `_meta["x.ai/tool"].kind` while title mutates
 * (`enter_plan_mode` → "Plan: Enter" → "Plan mode entered").
 */
export function inferToolNameFromMeta(
  title: string | null | undefined,
  meta: unknown,
): string {
  const m =
    meta && typeof meta === "object" && !Array.isArray(meta)
      ? (meta as Record<string, unknown>)
      : null;
  const xai = m?.["x.ai/tool"];
  if (xai && typeof xai === "object" && !Array.isArray(xai)) {
    const tool = xai as Record<string, unknown>;
    const kind = typeof tool.kind === "string" ? tool.kind.toLowerCase() : "";
    if (kind === "enter_plan") return "enter_plan_mode";
    if (kind === "exit_plan") return "exit_plan_mode";
    if (kind === "ask_user") return "ask_user_question";
    if (typeof tool.name === "string" && tool.name.trim()) {
      return normalizeToolName(tool.name);
    }
  }
  return normalizeToolName(title);
}

export function isPlanModeToolName(name: string): boolean {
  const n = normalizeToolName(name);
  return (
    n === "enter_plan_mode" ||
    n === "exit_plan_mode" ||
    n === "enterplanmode" ||
    n === "exitplanmode"
  );
}
