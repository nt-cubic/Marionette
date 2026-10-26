/**
 * Memo inbox client (edge dock stage 3, docs/plan-edge-dock.md §4).
 *
 * Deterministic local storage behind four Rust commands — files are COPIED
 * into ~/.marionette/memos (content-hash dedup), text/links land in the
 * index. This module is transport only; MemoBox owns the list state.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./api";

export type MemoKind = "file" | "text" | "image" | "link";

export type MemoEntry = {
  id: string;
  kind: MemoKind;
  title: string;
  path?: string;
  content?: string;
  size?: number;
  /** Unix ms since epoch, as a string. */
  createdAt: string;
  source?: string;
};

/** dataTransfer type marking an in-page memo drag (Composer accepts these). */
export const MEMO_DRAG_TYPE = "application/x-marionette-memo";

export async function memoList(): Promise<MemoEntry[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<MemoEntry[]>("memo_list");
  } catch {
    return [];
  }
}

/** Explorer drag-in (absolute paths). Returns the full refreshed list. */
export async function memoSavePaths(paths: string[], source?: string): Promise<MemoEntry[]> {
  return await invoke<MemoEntry[]>("memo_save_paths", { paths, source });
}

/** Ctrl+V channel: text, or a bare URL which becomes a link entry. */
export async function memoSaveText(text: string, title?: string, source?: string): Promise<MemoEntry | null> {
  try {
    return await invoke<MemoEntry>("memo_save_text", { text, title, source });
  } catch {
    return null;
  }
}

/** Returns the refreshed list, or null when the delete failed (keep what is shown). */
export async function memoDelete(id: string): Promise<MemoEntry[] | null> {
  try {
    return await invoke<MemoEntry[]>("memo_delete", { id });
  } catch {
    return null;
  }
}

/** `1200000` → `1.2 MB`; folders pass through dir_size bytes. */
export function formatMemoSize(size?: number): string {
  if (typeof size !== "number" || size <= 0) return "";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${Math.round(size / 102.4) / 10} KB`;
  return `${Math.round(size / (102.4 * 1024)) / 10} MB`;
}

/** Unix-ms string → `刚刚 / 5 分钟前 / 昨天 / 10-24 / 2025-10-24`. */
export function formatMemoAge(createdAt: string, now = Date.now()): string {
  const ms = Number(createdAt);
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const diff = now - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const date = new Date(ms);
  const today = new Date(now);
  const sameYear = date.getFullYear() === today.getFullYear();
  const pad = (n: number) => String(n).padStart(2, "0");
  return sameYear
    ? `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
    : `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

export type MemoImageOrient = "landscape" | "portrait" | "square";

/**
 * Chat-app image box (Telegram / Discord / iMessage): pick which side of the
 * card the bitmap fills, then scale the other side so the whole picture
 * stays visible. 5:4 and wider is landscape, 4:5 and taller is portrait.
 */
export function memoImageOrient(width: number, height: number): MemoImageOrient {
  if (!(width > 0) || !(height > 0)) return "square";
  const ratio = width / height;
  if (ratio >= 5 / 4) return "landscape";
  if (ratio <= 4 / 5) return "portrait";
  return "square";
}

/** Host of a link memo (`www.` stripped). Bad URLs fall back to the raw string. */
export function memoLinkHost(content?: string): string {
  if (!content) return "";
  try {
    return new URL(content).host.replace(/^www\./i, "");
  } catch {
    return content;
  }
}

export type MemoDayGroup = {
  key: string;
  label: string;
  entries: MemoEntry[];
};

function localDayKey(ms: number): string {
  const date = new Date(ms);
  return `${date.getFullYear()}-${date.getMonth() + 1}-${date.getDate()}`;
}

function localDayLabel(ms: number, now: number): string {
  const date = new Date(ms);
  const today = new Date(now);
  const startToday = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  const startThat = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const diffDays = Math.round((startToday - startThat) / 86_400_000);
  if (diffDays === 0) return "今天";
  if (diffDays === 1) return "昨天";
  if (date.getFullYear() === today.getFullYear()) {
    return `${date.getMonth() + 1}月${date.getDate()}日`;
  }
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

/**
 * Consecutive day buckets for a newest-first list. Invalid timestamps keep
 * their own unlabeled group so they still render.
 */
export function groupMemosByDay(entries: MemoEntry[], now = Date.now()): MemoDayGroup[] {
  const groups: MemoDayGroup[] = [];
  for (const entry of entries) {
    const ms = Number(entry.createdAt);
    const valid = Number.isFinite(ms) && ms > 0;
    const key = valid ? localDayKey(ms) : `unknown:${entry.id}`;
    const last = groups[groups.length - 1];
    if (last && last.key === key) {
      last.entries.push(entry);
      continue;
    }
    groups.push({
      key,
      label: valid ? localDayLabel(ms, now) : "",
      entries: [entry],
    });
  }
  return groups;
}

/**
 * Composer text for picked memos (§4.1 收/找/用). `dialog` hands the items to a
 * new dialog as context; `doc` asks the agent to organize them into a note
 * under the project's docs/notes/. Prefilled only — the user sends it.
 */
export function memoPrompt(entries: MemoEntry[], intent: "dialog" | "doc"): string {
  const lines = entries.map((entry) => {
    if (entry.kind === "link") return `- 链接：${entry.title} ${entry.content ?? ""}`.trimEnd();
    if (entry.kind === "text") {
      const body = (entry.content ?? entry.title).trim().replace(/\n/g, "\n  > ");
      return `- 摘录：\n  > ${body}`;
    }
    return `- ${entry.kind === "image" ? "图片" : "文件"}：${entry.path ?? entry.title}`;
  });
  const list = lines.join("\n");
  if (intent === "doc") {
    const date = new Date().toISOString().slice(0, 10);
    return [
      `把下面 ${entries.length} 条收纳盒材料整理成一篇笔记：按主题分节，每节先给结论再列出处；链接保留原 URL，文件按需读取。`,
      `存到项目的 docs/notes/${date}-<主题>.md，写完告诉我文件路径。`,
      "",
      list,
    ].join("\n");
  }
  return [`参考下面 ${entries.length} 条材料：`, "", list, "", ""].join("\n");
}
