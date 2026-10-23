/**
 * Inbox transcript → Memos-style timeline.
 *
 * Captures are stored as user_message events on the 收集箱 session. The send
 * path wraps them in an organizing prompt; this module unwraps that so the
 * page shows the note the user actually wrote.
 */

import type { ImageAttachment } from "./imageAttachments";
import type { SessionEvent } from "./types";

export type MemoItem = {
  id: string;
  createdAt: string;
  text: string;
  source?: string;
  attachments?: ImageAttachment[];
  tags: string[];
};

export type MemoDayGroup = {
  day: string;
  items: MemoItem[];
};

const ORGANIZE_HEAD = "新收藏进来了，请整理进知识库：";
const ORGANIZE_TAIL = "\n要求：";
const SOURCE_LINE = /^来源：(.+?)(?:\s*·\s*(.+))?$/;

export function parseTags(text: string): string[] {
  const found = text.match(/(^|\s)#([\p{L}\p{N}_/-]+)/gu);
  if (!found) return [];
  const tags: string[] = [];
  for (const raw of found) {
    const tag = raw.trim().replace(/^#/, "");
    if (tag && !tags.includes(tag)) tags.push(tag);
  }
  return tags;
}

export function unwrapCaptureText(raw: string): { text: string; source?: string } {
  let body = raw.trim();
  const headAt = body.indexOf(ORGANIZE_HEAD);
  if (headAt >= 0) {
    body = body.slice(headAt + ORGANIZE_HEAD.length);
    const tailAt = body.indexOf(ORGANIZE_TAIL);
    if (tailAt >= 0) body = body.slice(0, tailAt);
    body = body.trim();
  }
  const lines = body.split(/\r?\n/);
  let source: string | undefined;
  if (lines[0] && SOURCE_LINE.test(lines[0].trim())) {
    source = lines[0].trim().replace(/^来源：/, "");
    lines.shift();
    while (lines[0] === "") lines.shift();
  }
  return { text: lines.join("\n").trim(), source };
}

export function memosFromEvents(events: SessionEvent[]): MemoItem[] {
  const items: MemoItem[] = [];
  for (const event of events) {
    if (event.type !== "user_message") continue;
    const parsed = unwrapCaptureText(event.text ?? "");
    if (!parsed.text && !(event.attachments && event.attachments.length > 0)) continue;
    items.push({
      id: event.messageId ?? `${event.createdAt}-${items.length}`,
      createdAt: event.createdAt,
      text: parsed.text,
      source: parsed.source,
      attachments: event.attachments,
      tags: parseTags(parsed.text),
    });
  }
  items.reverse();
  return items;
}

function startOfLocalDay(iso: string): number {
  const d = new Date(iso);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

export function formatMemoDay(iso: string, now = Date.now()): string {
  const start = startOfLocalDay(iso);
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const diff = today.getTime() - start;
  if (diff === 0) return "今天";
  if (diff === 86_400_000) return "昨天";
  const d = new Date(iso);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function formatMemoTime(iso: string): string {
  const d = new Date(iso);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function groupMemosByDay(items: MemoItem[], now = Date.now()): MemoDayGroup[] {
  const groups: MemoDayGroup[] = [];
  for (const item of items) {
    const day = formatMemoDay(item.createdAt, now);
    const last = groups[groups.length - 1];
    if (last && last.day === day) last.items.push(item);
    else groups.push({ day, items: [item] });
  }
  return groups;
}
