import { useRef, type MutableRefObject } from "react";
import {
  applyAcpPartToEvents,
  coalesceAdjacentAssistantFragments,
  coalesceAdjacentThoughts,
  findLastIndexForSession,
  type AcpTextPart,
} from "../lib/acpTranscript";
import { isTauriRuntime, appendTranscriptEvents, writeTranscript } from "../lib/api";
import { persistableEventsForSession } from "../lib/transcript";
import type { SessionEvent } from "../lib/types";

const EMPTY_EVENTS: SessionEvent[] = [];

/** 基础 debounce；事件数组超过该规模后升档（见 FLUSH_DEBOUNCE_SLOW_MS）。 */
const FLUSH_DEBOUNCE_MS = 450;
const FLUSH_DEBOUNCE_SLOW_EVENTS = 3000;
const FLUSH_DEBOUNCE_SLOW_MS = 2000;

type SendMeta = {
  agentId?: string;
  agentLabel?: string;
  modelId?: string;
  modelLabel?: string;
  modeLabel?: string;
  effortLabel?: string;
} | null;

export interface SessionRailsEnv {
  transcriptSaveTimers: MutableRefObject<Map<string, ReturnType<typeof setTimeout>>>;
  ownsAcpSession: (sessionId: string) => boolean;
  reloadingSessionsRef: MutableRefObject<Set<string>>;
  pushDebugRef: MutableRefObject<(entry: {
    sessionId?: string;
    level?: "info" | "warn" | "error";
    source: string;
    summary: string;
    detail?: string;
  }) => void>;
  bumpEventsTick: (ids: string[]) => void;
  delegateMetaRef: MutableRefObject<Map<string, { parentId: string }>>;
  turnStartedAtRef: MutableRefObject<Record<string, number>>;
  sendMetaRef: MutableRefObject<SendMeta>;
  /** 会话的转录头部是否仍截断未加载（尾部优先加载）。 */
  isHeadTruncated: (sessionId: string) => boolean;
  /** 全量回写前补齐头部（内部会再触发 debounce，本轮 flush 让位）。 */
  ensureHeadForPersist: (sessionId: string) => Promise<void>;
}

/**
 * Per-session transcript rails (out of App.tsx, 审计「App.tsx 事件管线」):
 * 每个会话各持一条事件 rail（shadow = 落盘影子、live = 渲染用），流式分块
 * 经 applyStreamPart 合并进 rail，渲染视图再并入子会话流。全部挂在 ref
 * Map 上——React 状态只有每会话的 tick 计数（App 侧），rail 本体不整条
 * 进 state。
 */
export function useSessionRails(env: SessionRailsEnv) {
  const eventsBySessionRef = useRef(new Map<string, SessionEvent[]>());
  const shadowBySessionRef = useRef(new Map<string, SessionEvent[]>());
  const livePaintSetRef = useRef<Set<string>>(new Set());
  /**
   * 上次已落盘的（persistable）事件数组，按引用身份做前缀比对：前缀全等 +
   * 变长 = 纯追加 → 走 appendTranscriptEvents 只续文件尾，不再每 450ms 全量
   * 序列化 + 整文件重写（事件数组随历史线性膨胀，本项目最大转录 24MB，
   * 缺页风暴下每次全量重写都是秒级硬等待——2026-10-31 事故主因之一）。
   */
  const lastPersistedRef = useRef(new Map<string, SessionEvent[]>());

  const notePersisted = (sessionId: string, events: SessionEvent[]) => {
    lastPersistedRef.current.set(sessionId, events);
  };

  const armTranscriptTimer = (sessionId: string) => {
    if (!isTauriRuntime()) return;
    if (!env.ownsAcpSession(sessionId)) return;
    if (env.reloadingSessionsRef.current.has(sessionId)) return;
    const prev = env.transcriptSaveTimers.current.get(sessionId);
    if (prev) clearTimeout(prev);
    // 自适应 debounce：大转录（按事件数近似）流式期间升档，磁盘压力让路；
    // turn 收尾 / 关闭路径的显式 flush 不受影响。
    const sizeHint = shadowBySessionRef.current.get(sessionId)?.length ?? 0;
    const delay = sizeHint > FLUSH_DEBOUNCE_SLOW_EVENTS ? FLUSH_DEBOUNCE_SLOW_MS : FLUSH_DEBOUNCE_MS;
    const timer = setTimeout(() => {
      env.transcriptSaveTimers.current.delete(sessionId);
      if (!env.ownsAcpSession(sessionId)) return;
      if (env.reloadingSessionsRef.current.has(sessionId)) return;
      const events = persistableEventsForSession(
        shadowBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS,
        sessionId,
      );
      if (events.length === 0) return;
      const last = lastPersistedRef.current.get(sessionId);
      const prefixIdentical =
        last != null && events.length >= last.length && last.every((ev, i) => events[i] === ev);
      if (last != null && prefixIdentical) {
        if (events.length === last.length) return; // 无变化
        const suffix = events.slice(last.length);
        lastPersistedRef.current.set(sessionId, events);
        void appendTranscriptEvents(sessionId, suffix).catch((error) => {
          // 追加失败：丢弃账本标记，下一轮自动退回全量重写（保守恢复）。
          lastPersistedRef.current.delete(sessionId);
          env.pushDebugRef.current({
            sessionId,
            level: "warn",
            source: "transcript",
            summary: "append transcript failed; falling back to full write",
            detail: error instanceof Error ? error.message : String(error),
          });
        });
        return;
      }
      // 全量重写前必须补齐截断的头部——rail 只装了尾部，直接写会把磁盘上
      // 的历史头吃掉。ensureHead 内部的 updateSessionEvents 会重新 arm 本
      // timer，届时头已在、走真正的全量写。
      if (env.isHeadTruncated(sessionId)) {
        void env.ensureHeadForPersist(sessionId);
        return;
      }
      lastPersistedRef.current.set(sessionId, events);
      void writeTranscript(sessionId, events).catch((error) => {
        lastPersistedRef.current.delete(sessionId);
        env.pushDebugRef.current({
          sessionId,
          level: "warn",
          source: "transcript",
          summary: "write transcript failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      });
    }, delay);
    env.transcriptSaveTimers.current.set(sessionId, timer);
  };

  const updateSessionEvents = (
    sessionId: string,
    fn: (current: SessionEvent[]) => SessionEvent[],
    opts?: { publish?: boolean },
  ): SessionEvent[] => {
    const prev = shadowBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS;
    const next = fn(prev);
    if (next === prev) return prev;
    shadowBySessionRef.current.set(sessionId, next);
    armTranscriptTimer(sessionId);
    const publish = opts?.publish ?? livePaintSetRef.current.has(sessionId);
    if (publish) {
      eventsBySessionRef.current.set(sessionId, next);
      env.bumpEventsTick([sessionId]);
    }
    return next;
  };

  const dropSessionRails = (ids: Iterable<string>) => {
    const bumped: string[] = [];
    for (const id of ids) {
      if (shadowBySessionRef.current.has(id) || eventsBySessionRef.current.has(id)) {
        shadowBySessionRef.current.delete(id);
        eventsBySessionRef.current.delete(id);
        lastPersistedRef.current.delete(id);
        bumped.push(id);
      }
    }
    env.bumpEventsTick(bumped);
  };

  const applyStreamPart = (
    current: SessionEvent[],
    sid: string,
    extracted: AcpTextPart,
  ): SessionEvent[] => {
    const prevIdx = findLastIndexForSession(current, sid);
    const prevLast = prevIdx >= 0 ? current[prevIdx] : undefined;
    const prevLen = current.length;
    let next = applyAcpPartToEvents(current, sid, extracted);
    if (extracted.role === "assistant") {
      const newIdx = findLastIndexForSession(next, sid);
      const newLast = newIdx >= 0 ? next[newIdx] : undefined;
      if (
        newLast?.type === "assistant_message" &&
        newLast.sessionId === sid &&
        (prevLast?.type !== "assistant_message" || prevLast.sessionId !== sid) &&
        !env.turnStartedAtRef.current[sid]
      ) {
        env.turnStartedAtRef.current[sid] = Date.now();
        env.sendMetaRef.current = null;
      }
    }
    if (
      next.length > prevLen &&
      (extracted.role === "thought" || extracted.role === "assistant")
    ) {
      next = coalesceAdjacentThoughts(next, sid);
      if (extracted.role === "assistant") {
        next = coalesceAdjacentAssistantFragments(next, sid);
      }
    }
    return next;
  };

  const publishedEventsForView = (sessionId: string): SessionEvent[] => {
    const parent = eventsBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS;
    const childIds = new Set<string>();
    for (const [childId, meta] of env.delegateMetaRef.current) {
      if (meta.parentId === sessionId) childIds.add(childId);
    }
    for (const e of parent) {
      if (e.type === "subtask_started" || e.type === "subtask_result") {
        childIds.add(e.childSessionId);
      }
    }
    if (childIds.size === 0) return parent;
    const out = parent.slice();
    for (const id of childIds) {
      const rail = eventsBySessionRef.current.get(id);
      if (rail?.length) out.push(...rail);
    }
    return out;
  };

  return {
    eventsBySessionRef,
    shadowBySessionRef,
    livePaintSetRef,
    updateSessionEvents,
    dropSessionRails,
    applyStreamPart,
    publishedEventsForView,
    notePersisted,
  };
}
