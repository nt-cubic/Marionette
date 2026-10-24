import { useRef, type MutableRefObject } from "react";
import {
  applyAcpPartToEvents,
  coalesceAdjacentAssistantFragments,
  coalesceAdjacentThoughts,
  findLastIndexForSession,
  type AcpTextPart,
} from "../lib/acpTranscript";
import { isTauriRuntime, writeTranscript } from "../lib/api";
import { persistableEventsForSession } from "../lib/transcript";
import type { SessionEvent } from "../lib/types";

const EMPTY_EVENTS: SessionEvent[] = [];

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

  const armTranscriptTimer = (sessionId: string) => {
    if (!isTauriRuntime()) return;
    if (!env.ownsAcpSession(sessionId)) return;
    if (env.reloadingSessionsRef.current.has(sessionId)) return;
    const prev = env.transcriptSaveTimers.current.get(sessionId);
    if (prev) clearTimeout(prev);
    const timer = setTimeout(() => {
      env.transcriptSaveTimers.current.delete(sessionId);
      if (!env.ownsAcpSession(sessionId)) return;
      if (env.reloadingSessionsRef.current.has(sessionId)) return;
      const events = persistableEventsForSession(
        shadowBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS,
        sessionId,
      );
      void writeTranscript(sessionId, events).catch((error) => {
        env.pushDebugRef.current({
          sessionId,
          level: "warn",
          source: "transcript",
          summary: "write transcript failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      });
    }, 450);
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
  };
}
