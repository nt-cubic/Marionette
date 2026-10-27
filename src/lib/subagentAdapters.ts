/**
 * Per-agent nested-agent adapters (Codeg-aligned).
 *
 * ACP v1 has no subagent type. Each harness smuggles the spawn differently,
 * and several never put "subagent" on the wire — the card title is the task
 * description, the real name sits in `_meta`, or there is no tool_call at all.
 *
 *   Claude Code   Agent/Task + `_meta.claudeCode.toolName` / parentToolUseId
 *   OpenCode      `task` + `opencode/session/child_update`
 *   Grok Build    `spawn_subagent` + `_x.ai/session/update` (child stream stays off the parent)
 *   Codex         `collabAgentToolCall` (title spawnAgent/wait/…) OR
 *                 `_meta.codex.subagent` from subAgentActivity — native team-of-agents
 *                 often raises no named spawn tool
 *   ZCode         Agent/Task whose *title* is the prompt; identity in
 *                 `_meta.claudeCode.toolName` / `_meta.subagent`; background
 *                 children are `[background] …` cards (kind other)
 *   Hermes        `delegate_task`
 *   DeepSeek/DSH  `task` / `subagent`
 *   CodeBuddy     `_meta.isSubagent` / teamUpdate members (custom agent)
 *
 * We recover a stable tool name at parse time and treat the card as a nested
 * agent even when the protocol does not label it. Inner child transcripts are
 * only shown when the adapter actually forwards them.
 */

function asRecord(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function compact(name: string): string {
  return name.toLowerCase().replace(/[\s_-]+/g, "");
}

/** Codex collab / Grok wait / roster — not a nested-agent card of its own. */
const WAIT_EXACT = new Set([
  "wait",
  "waitagent",
  "close",
  "closeagent",
  "resume",
  "resumeagent",
  "sendinput",
  "interruptagent",
  "listagents",
  "list",
  "followuptask",
]);

export function isSubagentWaitTool(
  toolName: string | null | undefined,
  title?: string | null,
): boolean {
  const hay = `${toolName ?? ""} ${title ?? ""}`;
  if (
    /get_command_or_subagent_output|wait_agent|close_agent|background_task_action|multi-wait|wait_all|task output|interrupt_agent|send_input|followup_task|resume_agent|list_agents/i.test(
      hay,
    )
  ) {
    return true;
  }
  return WAIT_EXACT.has(compact(toolName ?? "")) || WAIT_EXACT.has(compact(title ?? ""));
}

const SPAWN_EXACT = new Set([
  "task",
  "agent",
  "subagent",
  "delegate",
  "delegatetask",
  "delegate_task",
  "delegate_to_agent",
  "delegatetoagent",
  "dispatch_agent",
  "dispatchagent",
  "spawn_agent",
  "spawnagent",
  "spawn_subagent",
  "spawnsubagent",
  "collab_agent",
  "collabagent",
  "call_omo_agent",
  "callomoagent",
  "invoke_agent",
  "invokeagent",
  "run_agent",
  "runagent",
  "launch_agent",
  "launchagent",
  "background_agent",
  "backgroundagent",
  "team_create",
  "teamcreate",
]);

function looksLikeSpawnName(name: string): boolean {
  const n = name.trim();
  if (!n || /todo/i.test(n)) return false;
  const c = compact(n);
  if (SPAWN_EXACT.has(c) || SPAWN_EXACT.has(n.toLowerCase())) return true;
  if (/^(task|agent|subagent)$/i.test(n)) return true;
  return /subagent|spawn_agent|spawnagent|dispatch_agent|invoke_agent|launch_agent|delegate_task|collab_agent/i.test(
    n,
  );
}

function looksLikeSpawnTitle(title: string): boolean {
  const t = title.trim();
  if (!t) return false;
  if (/^\[background\]/i.test(t)) return true;
  if (/^spawnagent$/i.test(t) || /^spawn_agent$/i.test(t)) return true;
  if (/\b(subagent|sub-agent|nested agent|task agent|delegate agent)\b/i.test(t)) return true;
  if (/子代理|子\s*agent/i.test(t)) return true;
  if (/^task\b/i.test(t)) return true;
  if (/\(general\)|\(explore\)|\(scout\)/i.test(t)) return true;
  return false;
}

/** Codex collab rawInput: senderThreadId + receiverThreadIds + agentsStates. */
export function isCodexCollabInput(input: string | null | undefined): boolean {
  const rec = parseObject(input);
  if (!rec) return false;
  return "senderThreadId" in rec && "receiverThreadIds" in rec && "agentsStates" in rec;
}

function parseObject(input: string | null | undefined): Record<string, unknown> | null {
  if (!input) return null;
  const trimmed = input.trim();
  if (!trimmed.startsWith("{")) return null;
  try {
    return asRecord(JSON.parse(trimmed));
  } catch {
    return null;
  }
}

/** Claude Agent / Grok task / ZCode Agent input that never says "subagent". */
export function inputLooksLikeSubagentSpawn(input: string | null | undefined): boolean {
  if (isCodexCollabInput(input)) return true;
  const rec = parseObject(input);
  if (!rec) {
    const raw = (input ?? "").trim();
    if (!raw) return false;
    if (/subagent_id:\s*\S+/i.test(raw) && /description:/i.test(raw)) return true;
    if (/working in the background/i.test(raw) && /agent[_-]?id/i.test(raw)) return true;
    return false;
  }
  if (rec.subagent_type != null || rec.subagentType != null) return true;
  if (rec.run_in_background === true || rec.runInBackground === true) {
    if (rec.prompt != null || rec.description != null || rec.goal != null) return true;
  }
  if (typeof rec.goal === "string" && (rec.role === "leaf" || rec.toolsets != null)) return true;
  if (rec.agentsStates != null && rec.senderThreadId != null) return true;
  return false;
}

export function isSubagentTool(
  toolName: string | null | undefined,
  title?: string | null,
  input?: string | null,
): boolean {
  if (isSubagentWaitTool(toolName, title)) return false;
  if (looksLikeSpawnName(toolName ?? "")) return true;
  if (looksLikeSpawnTitle(title ?? "")) return true;
  if (inputLooksLikeSubagentSpawn(input)) return true;
  return false;
}

function metaToolName(meta: unknown): string | null {
  const m = asRecord(meta);
  if (!m) return null;
  const claude = asRecord(m.claudeCode) ?? asRecord(m["claude-code"]);
  if (typeof claude?.toolName === "string" && claude.toolName.trim()) return claude.toolName.trim();
  if (typeof claude?.name === "string" && claude.name.trim()) return claude.name.trim();
  if (m.subagent && typeof m.subagent === "object") return "agent";
  const zcode = asRecord(m.zcode);
  if (zcode?.collapsed === "task-notification") return "agent";
  if (typeof zcode?.toolName === "string" && zcode.toolName.trim()) return zcode.toolName.trim();
  const codex = asRecord(m.codex);
  if (codex?.subagent) return "agent";
  if (codex?.collaboration) return "collab_agent";
  if (m.isSubagent === true || m["codebuddy.ai/isSubagent"] === true) return "agent";
  if (m.backgroundTask && typeof m.backgroundTask === "object") return "agent";
  const xai = asRecord(m["x.ai/tool"]);
  if (typeof xai?.name === "string" && looksLikeSpawnName(xai.name)) return xai.name;
  if (typeof xai?.kind === "string" && looksLikeSpawnName(xai.kind)) return xai.kind;
  return null;
}

function canonicalSpawnName(raw: string): string {
  const c = compact(raw);
  if (c === "task" || c === "tasktool") return "task";
  if (c.includes("spawnsubagent")) return "spawn_subagent";
  if (c.includes("collab")) return "collab_agent";
  if (c.includes("delegatetask") || c === "delegatetask") return "delegate_task";
  return "agent";
}

/**
 * Stable spawn identity for a tool_call update. Prefers vendor `_meta` over
 * the human title (ZCode/Claude put "Agent" in meta and the prompt in title).
 */
export function recoverSubagentToolName(args: {
  name?: string | null;
  title?: string | null;
  kind?: string | null;
  input?: string | null;
  meta?: unknown;
}): string | null {
  const fromMeta = metaToolName(args.meta);
  if (fromMeta && (looksLikeSpawnName(fromMeta) || isSubagentTool(fromMeta, args.title, args.input))) {
    return canonicalSpawnName(fromMeta);
  }
  const wire = (args.name ?? "").trim();
  if (wire && isSubagentTool(wire, args.title, args.input)) {
    return canonicalSpawnName(wire);
  }
  if (isCodexCollabInput(args.input)) {
    if (isSubagentWaitTool(args.name, args.title)) return null;
    return "collab_agent";
  }
  if (looksLikeSpawnTitle(args.title ?? "")) {
    const t = (args.title ?? "").trim();
    if (/^task\b/i.test(t)) return "task";
    return "agent";
  }
  if (inputLooksLikeSubagentSpawn(args.input)) return "agent";
  const kind = (args.kind ?? "").trim();
  if (kind && looksLikeSpawnName(kind)) return canonicalSpawnName(kind);
  return null;
}

export type CodexSubagentActivity = {
  toolCallId: string;
  title: string;
  status: string;
  detail: string;
  threadId?: string;
};

/** Codex native team-of-agents (`_meta.codex.subagent`) when no spawn tool exists. */
export function parseCodexSubagentActivity(update: Record<string, unknown>): CodexSubagentActivity | null {
  const meta = asRecord(update._meta) ?? asRecord(update.meta);
  const codex = meta ? asRecord(meta.codex) : null;
  const sub = codex ? asRecord(codex.subagent) : asRecord(update.subagent);
  if (!sub) return null;
  const threadId =
    (typeof sub.threadId === "string" && sub.threadId.trim()) ||
    (typeof sub.agentThreadId === "string" && sub.agentThreadId.trim()) ||
    (typeof sub.agent_thread_id === "string" && sub.agent_thread_id.trim()) ||
    "";
  const path =
    (typeof sub.agentPath === "string" && sub.agentPath.trim()) ||
    (typeof sub.agent_path === "string" && sub.agent_path.trim()) ||
    (typeof sub.task === "string" && sub.task.trim()) ||
    (typeof sub.title === "string" && sub.title.trim()) ||
    "";
  const activity =
    (typeof sub.activityKind === "string" && sub.activityKind.trim()) ||
    (typeof sub.kind === "string" && sub.kind.trim()) ||
    (typeof sub.activity === "string" && sub.activity.trim()) ||
    "";
  const id =
    (typeof update.toolCallId === "string" && update.toolCallId.trim()) ||
    (typeof sub.id === "string" && sub.id.trim()) ||
    threadId ||
    "";
  if (!id && !path && !activity) return null;
  const statusRaw =
    (typeof sub.status === "string" && sub.status.trim()) ||
    (typeof update.status === "string" && update.status.trim()) ||
    activity ||
    "running";
  const running = /run|progress|pending|start|init/i.test(statusRaw);
  const failed = /fail|error|interrupt/i.test(statusRaw);
  const status = failed ? "failed" : running ? "in_progress" : /complete|done|idle|shutdown/i.test(statusRaw) ? "completed" : "in_progress";
  const title = path || activity || "Subagent";
  const detailBits = [threadId && `thread: ${threadId}`, activity && `activity: ${activity}`].filter(Boolean);
  return {
    toolCallId: id || `codex-subagent-${title}`,
    title,
    status,
    detail: detailBits.join("\n"),
    threadId: threadId || undefined,
  };
}

export type CodeBuddyTeamMember = {
  toolCallId: string;
  title: string;
  status: string;
  detail: string;
};

/** CodeBuddy `_meta['codebuddy.ai/teamUpdate']` members — no per-child tool_call. */
export function parseCodeBuddyTeamMembers(update: Record<string, unknown>): CodeBuddyTeamMember[] {
  const meta = asRecord(update._meta) ?? asRecord(update.meta);
  if (!meta) return [];
  const team = asRecord(meta["codebuddy.ai/teamUpdate"]) ?? asRecord(meta.teamUpdate);
  if (!team || team.type === "team_deleted") return [];
  const members = Array.isArray(team.members) ? team.members : [];
  const rows: CodeBuddyTeamMember[] = [];
  for (const raw of members) {
    const m = asRecord(raw);
    if (!m) continue;
    const name = typeof m.name === "string" ? m.name.trim() : "";
    const taskId = typeof m.taskId === "string" ? m.taskId.trim() : "";
    const sessionId = typeof m.sessionId === "string" ? m.sessionId.trim() : "";
    const description = typeof m.description === "string" ? m.description.trim() : "";
    const statusRaw = typeof m.status === "string" ? m.status.trim() : "running";
    if (!name && !taskId) continue;
    const running = /run|progress|pending/i.test(statusRaw);
    const failed = /fail|error/i.test(statusRaw);
    rows.push({
      toolCallId: taskId || sessionId || `codebuddy-${name}`,
      title: name || description || "Subagent",
      status: failed ? "failed" : running ? "in_progress" : "completed",
      detail: [description, taskId && `taskId: ${taskId}`].filter(Boolean).join("\n"),
    });
  }
  return rows;
}

export function extractNestedAgentId(text: string | null | undefined): string | null {
  const raw = (text ?? "").trim();
  if (!raw) return null;
  const rec = parseObject(raw);
  if (rec) {
    for (const key of ["subagent_id", "subagentId", "agentId", "agent_id", "taskId", "threadId"]) {
      const v = rec[key];
      if (typeof v === "string" && v.trim()) return v.trim();
    }
  }
  const line =
    raw.match(/subagent_id:\s*(\S+)/i)?.[1] ||
    raw.match(/agentId:\s*(\S+)/i)?.[1] ||
    raw.match(/agent_id:\s*(\S+)/i)?.[1] ||
    raw.match(/taskId:\s*(\S+)/i)?.[1];
  return line?.trim() || null;
}
