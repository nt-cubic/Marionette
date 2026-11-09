/**
 * seenImages — every image this conversation has actually put in front of the agent.
 *
 * Two sources, both already in the transcript events (no new storage, no reload):
 *   - `user_message.attachments`  — images the user attached ("you")
 *   - `tool_call` images          — files a tool read, browser / Computer Use screenshots
 *
 * Honesty: this is what we *sent / received*, not proof the model looked at it. Whether
 * the current model can take images at all is `imageSupport.ts` (three-state, reported by
 * the agent or the model catalog — never guessed from a model name).
 */

import type { SessionEvent } from "./types";
import { collectToolImageSrcs } from "./imageAttachments";
import { resolveToolImagePath } from "./toolBody";
import { IMAGE_TOKENS_EST } from "./usageDigest";

/** How an image got into the conversation. */
export type SeenVia = "you" | "browser" | "computer" | "read";

/** Enough to find the card again (SessionView resolves it against the live events). */
export type SeenImageRef =
  | { type: "user_message"; messageId?: string; createdAt: string }
  | { type: "tool_call"; toolCallId?: string; createdAt: string };

export type SeenImage = {
  /** Normalised path / URL: the same picture read twice is one entry. */
  key: string;
  /** Resolved local path (cwd applied) or http(s) URL — what the viewer loads. */
  src: string;
  /** File name for captions. */
  name: string;
  via: SeenVia;
  /** Raw tool name of the latest sighting (browser_take_screenshot, read, …). */
  toolName?: string;
  /** Latest sighting, epoch ms. */
  at: number;
  /** How many times it was shown to the agent. */
  uses: number;
  /** The card of the latest sighting. */
  ref: SeenImageRef;
};

export type SeenFilter = "all" | "you" | "tool";

const REMOTE = /^(?:https?|data|blob):/i;

function normKey(src: string): string {
  return src.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

function baseName(src: string): string {
  const clean = src.split(/[?#]/)[0] ?? src;
  return clean.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || src;
}

/** Which kind of tool produced / read the image. Name-based on the *tool*, never on the model. */
export function toolVia(toolName?: string, title?: string): SeenVia {
  const name = `${toolName ?? ""} ${title ?? ""}`.toLowerCase();
  if (/computer|desktop|screen_?capture|cua\b/.test(name)) return "computer";
  if (/browser|playwright|puppeteer|chrome|webview|navigate/.test(name)) return "browser";
  return "read";
}

function ms(at: string | undefined): number {
  if (!at) return 0;
  const v = Date.parse(at);
  return Number.isFinite(v) ? v : 0;
}

/** Newest first. One entry per distinct picture; later sightings overwrite time / card / via. */
export function collectSeenImages(events: readonly SessionEvent[], cwd?: string | null): SeenImage[] {
  const byKey = new Map<string, SeenImage>();
  const upsert = (src: string, name: string, via: SeenVia, toolName: string | undefined, at: number, ref: SeenImageRef) => {
    const key = normKey(src);
    const prev = byKey.get(key);
    if (!prev) {
      byKey.set(key, { key, src, name, via, ...(toolName ? { toolName } : {}), at, uses: 1, ref });
      return;
    }
    prev.uses += 1;
    if (at >= prev.at) {
      prev.at = at;
      prev.via = via;
      prev.ref = ref;
      if (toolName) prev.toolName = toolName;
      else delete prev.toolName;
    }
  };
  for (const event of events) {
    if (event.type === "user_message") {
      for (const att of event.attachments ?? []) {
        const src = att.path?.trim();
        if (!src) continue;
        upsert(src, att.name || baseName(src), "you", undefined, ms(event.sentAt ?? event.createdAt), {
          type: "user_message",
          ...(event.messageId ? { messageId: event.messageId } : {}),
          createdAt: event.createdAt,
        });
      }
    } else if (event.type === "tool_call") {
      for (const raw of collectToolImageSrcs(event)) {
        const src = REMOTE.test(raw) ? raw : resolveToolImagePath(raw, cwd);
        if (!src) continue;
        upsert(src, baseName(src), toolVia(event.toolName, event.title), event.toolName, ms(event.completedAt ?? event.createdAt), {
          type: "tool_call",
          ...(event.toolCallId ? { toolCallId: event.toolCallId } : {}),
          createdAt: event.createdAt,
        });
      }
    }
  }
  return [...byKey.values()].sort((a, b) => b.at - a.at);
}

export function filterSeen(images: readonly SeenImage[], filter: SeenFilter): SeenImage[] {
  if (filter === "all") return [...images];
  return images.filter((image) => (filter === "you" ? image.via === "you" : image.via !== "you"));
}

export function countSeenBySource(images: readonly SeenImage[]): { all: number; you: number; tool: number } {
  let you = 0;
  for (const image of images) if (image.via === "you") you += 1;
  return { all: images.length, you, tool: images.length - you };
}

/** ≈ tokens of the whole set (same flat rate the Token page uses; an estimate, not a measurement). */
export function estimateSeenTokens(count: number): number {
  return count * IMAGE_TOKENS_EST;
}

export type SeenGroupKind = "now" | "today" | "yesterday" | "day";
export type SeenGroup = {
  /** Stable key: `now`, `today`, `yesterday`, or `d:YYYY-MM-DD`. */
  key: string;
  kind: SeenGroupKind;
  /** For `day`: month (1–12) and day-of-month, for an `M/D` label. */
  month?: number;
  day?: number;
  items: SeenImage[];
};

function dayStart(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** Newest-first images → 刚刚 (≤10 min) / 今天 / 昨天 / one group per older day. Order preserved. */
export function groupSeenByAge(images: readonly SeenImage[], now: Date = new Date()): SeenGroup[] {
  const today = dayStart(now);
  const yesterday = today - 86_400_000;
  const groups: SeenGroup[] = [];
  const push = (key: string, kind: SeenGroupKind, image: SeenImage, month?: number, day?: number) => {
    const last = groups[groups.length - 1];
    if (last && last.key === key) {
      last.items.push(image);
      return;
    }
    groups.push({ key, kind, ...(month != null ? { month, day } : {}), items: [image] });
  };
  for (const image of images) {
    const age = now.getTime() - image.at;
    if (age <= 10 * 60_000) push("now", "now", image);
    else if (image.at >= today) push("today", "today", image);
    else if (image.at >= yesterday) push("yesterday", "yesterday", image);
    else {
      const d = new Date(image.at);
      const key = `d:${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      push(key, "day", image, d.getMonth() + 1, d.getDate());
    }
  }
  return groups;
}

/** `06:18` in the viewer's local clock. */
export function formatClock(at: number): string {
  const d = new Date(at);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** `11/9 06:18` when not today, else `06:18`. */
export function formatSeenTime(at: number, now: Date = new Date()): string {
  const d = new Date(at);
  return dayStart(d) === dayStart(now) ? formatClock(at) : `${d.getMonth() + 1}/${d.getDate()} ${formatClock(at)}`;
}

/** Find the live event a SeenImage's ref points at (index into `events`), or -1. */
export function findSeenEventIndex(events: readonly SessionEvent[], ref: SeenImageRef): number {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (ref.type === "user_message") {
      if (event.type !== "user_message") continue;
      if (ref.messageId ? event.messageId === ref.messageId : event.createdAt === ref.createdAt) return i;
    } else if (event.type === "tool_call") {
      if (ref.toolCallId ? event.toolCallId === ref.toolCallId : event.createdAt === ref.createdAt) return i;
    }
  }
  return -1;
}
