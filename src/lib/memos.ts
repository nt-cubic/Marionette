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
export function formatMemoAge(createdAt: string): string {
  const ms = Number(createdAt);
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const date = new Date(ms);
  const today = new Date();
  const sameYear = date.getFullYear() === today.getFullYear();
  const pad = (n: number) => String(n).padStart(2, "0");
  return sameYear
    ? `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
    : `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
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
