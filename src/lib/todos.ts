import type { PlanEntry } from "./acpPlan";

export type TodoStatus = "todo" | "doing" | "done";
export type TodoSource = "user" | "plan" | "ai";

export type TodoItem = {
  id: string;
  text: string;
  status: TodoStatus;
  source: TodoSource;
  /** source=plan 时记录来自哪个会话 */
  originSessionId?: string;
  createdAt: string;
  updatedAt: string;
  doneAt?: string;
};

export type TodoFile = {
  version: 1;
  items: TodoItem[];
  updatedAt: string;
};

/** 去首尾空白、去 markdown 标记、去尾部标点、转小写、取前 40 字 */
export function todoKey(text: string): string {
  let s = text.trim();
  s = s.replace(/\*\*([^*]+)\*\*/g, "$1");
  s = s.replace(/\*([^*]+)\*/g, "$1");
  s = s.replace(/`([^`]+)`/g, "$1");
  s = s.replace(/^#{1,6}\s+/, "");
  s = s.replace(/^\s*[-*+]\s+/, "");
  s = s.replace(/^\s*\d+[.)、]\s+/, "");
  s = s.replace(/[。．.!?？！,，;；:：]+$/g, "");
  s = s.toLowerCase().replace(/\s+/g, " ").trim();
  return [...s].slice(0, 40).join("");
}

export function newTodoId(): string {
  const t = Date.now().toString(36);
  const r = Math.random().toString(36).slice(2, 8);
  return `todo-${t}-${r}`;
}

function nowIso(): string {
  return new Date().toISOString();
}

export function planStatusToTodo(status: PlanEntry["status"]): TodoStatus {
  switch (status) {
    case "completed":
      return "done";
    case "in_progress":
      return "doing";
    default:
      return "todo";
  }
}

/** Absorb plan entries into project todos (dedupe by todoKey). */
export function absorbPlanIntoTodos(
  items: TodoItem[],
  plan: PlanEntry[],
  originSessionId: string,
): TodoItem[] {
  const now = nowIso();
  const next = [...items];
  const byKey = new Map(next.map((it) => [todoKey(it.text), it]));

  for (const entry of plan) {
    const key = todoKey(entry.content);
    if (!key) continue;
    const status = planStatusToTodo(entry.status);
    const existing = byKey.get(key);
    if (existing) {
      existing.status = status;
      existing.updatedAt = now;
      if (status === "done" && !existing.doneAt) existing.doneAt = now;
      if (status !== "done") existing.doneAt = undefined;
      // Keep user-edited text; only refresh origin if still from plan.
      if (existing.source === "plan") {
        existing.originSessionId = originSessionId;
      }
    } else {
      const item: TodoItem = {
        id: newTodoId(),
        text: entry.content.trim(),
        status,
        source: "plan",
        originSessionId,
        createdAt: now,
        updatedAt: now,
        doneAt: status === "done" ? now : undefined,
      };
      next.push(item);
      byKey.set(key, item);
    }
  }
  return next;
}

export function formatTodosForPrompt(items: TodoItem[]): string {
  if (items.length === 0) return "当前项目任务清单：（空）";
  const lines = items.map((it, i) => {
    const mark = it.status === "done" ? "x" : it.status === "doing" ? "~" : " ";
    return `${i + 1}. [${mark}] ${it.text}`;
  });
  return `当前项目任务清单：\n${lines.join("\n")}`;
}

export function formatAiUpdatePrompt(items: TodoItem[]): string {
  return [
    "这是当前项目的任务清单：",
    formatTodosForPrompt(items).replace(/^当前项目任务清单：\n?/, "") || "（空）",
    "",
    "请结合我们刚才的进展更新它：标记已完成的、补上新发现的。",
    "如果你有任务清单工具（TodoWrite / plan），请直接用它输出更新后的完整清单。",
    "否则请只回一个代码块：",
    "",
    "```marionette-todo",
    '[{"text":"…","status":"todo|doing|done"}]',
    "```",
  ].join("\n");
}

export type TodoMergePreview = {
  added: TodoItem[];
  completed: TodoItem[];
  statusChanged: TodoItem[];
  untouched: TodoItem[];
  /** Result after apply */
  nextItems: TodoItem[];
};

/**
 * Merge AI-proposed items by normalized text key.
 * Local items AI didn't mention are kept.
 */
export function previewMergeFromAi(
  items: TodoItem[],
  proposed: Array<{ text: string; status: TodoStatus }>,
): TodoMergePreview | { error: string } {
  if (proposed.length > 50) {
    return { error: "AI 返回条目过多（>50），已拒绝" };
  }
  const now = nowIso();
  const next = items.map((it) => ({ ...it }));
  const byKey = new Map(next.map((it) => [todoKey(it.text), it]));
  const mentioned = new Set<string>();
  const added: TodoItem[] = [];
  const completed: TodoItem[] = [];
  const statusChanged: TodoItem[] = [];

  for (const p of proposed) {
    const text = p.text.trim();
    if (!text) continue;
    const key = todoKey(text);
    if (!key) continue;
    mentioned.add(key);
    const existing = byKey.get(key);
    if (existing) {
      if (existing.status !== p.status) {
        const prev = existing.status;
        existing.status = p.status;
        existing.updatedAt = now;
        if (p.status === "done") {
          existing.doneAt = now;
          completed.push(existing);
        } else {
          existing.doneAt = undefined;
          statusChanged.push(existing);
        }
        if (prev === "done" && p.status !== "done") {
          statusChanged.push(existing);
        }
      }
    } else {
      const item: TodoItem = {
        id: newTodoId(),
        text,
        status: p.status,
        source: "ai",
        createdAt: now,
        updatedAt: now,
        doneAt: p.status === "done" ? now : undefined,
      };
      next.push(item);
      byKey.set(key, item);
      added.push(item);
    }
  }

  const untouched = next.filter((it) => !mentioned.has(todoKey(it.text)));
  return { added, completed, statusChanged, untouched, nextItems: next };
}

/** Parse ```marionette-todo ... ``` fenced block from assistant text. */
export function parseMarionetteTodoFence(
  text: string,
): Array<{ text: string; status: TodoStatus }> | null {
  const m = text.match(/```marionette-todo\s*([\s\S]*?)```/i);
  if (!m) return null;
  try {
    const raw = JSON.parse(m[1].trim()) as unknown;
    if (!Array.isArray(raw)) return null;
    const out: Array<{ text: string; status: TodoStatus }> = [];
    for (const item of raw) {
      if (!item || typeof item !== "object") continue;
      const rec = item as Record<string, unknown>;
      const t = typeof rec.text === "string" ? rec.text.trim() : "";
      if (!t) continue;
      const st = String(rec.status ?? "todo").toLowerCase();
      const status: TodoStatus =
        st === "done" || st === "completed" ? "done" : st === "doing" || st === "in_progress" ? "doing" : "todo";
      out.push({ text: t, status });
    }
    return out;
  } catch {
    return null;
  }
}

/** Convert plan entries into the same shape as AI proposed items. */
export function planToProposed(
  plan: PlanEntry[],
): Array<{ text: string; status: TodoStatus }> {
  return plan.map((e) => ({
    text: e.content,
    status: planStatusToTodo(e.status),
  }));
}

export function createTodo(text: string): TodoItem {
  const now = nowIso();
  return {
    id: newTodoId(),
    text: text.trim(),
    status: "todo",
    source: "user",
    createdAt: now,
    updatedAt: now,
  };
}
