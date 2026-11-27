/**
 * P0-2 ownership (backend-driven worker transcripts).
 *
 * One worker event chain has four distinct representations. Do not conflate
 * them, and do not add a second writer:
 *
 *   1. RAW ACP EVENTS -- JSON-RPC envelopes parsed in `src-tauri/src/acp.rs`
 *      (`session/update` etc). They feed in-process dispatch and dev.log only.
 *   2. TEE EVENTS -- `src-tauri/src/chief/tee.rs` appends those raw envelopes,
 *      unchanged, plus `{seq, ts, sessionId, kind}`, to
 *      `~/.marionette/chief/sessions/<sid>.events.jsonl`. This is an
 *      append-only RAW ledger, not SessionEvents. Its `consume` cursor is only
 *      a replay filter; it never deletes evidence.
 *   3. NORMALIZED SessionEvents -- `applyAcpPartToEvents` / `applyStreamPart`
 *      fold ACP chunks into cards (assistant / thought / tool / user_message).
 *      They live in the shadow/live rails here; this is the only shape that is
 *      rendered or persisted.
 *   4. CANONICAL TRANSCRIPT JSONL -- `<sid>.jsonl` with one persistable
 *      SessionEvent per line. **Only `persistSessionNow` in this file writes
 *      it** (live stream and tee replay share the same function). `storage.rs`,
 *      `chief/broker.rs` and `chief/tee.rs` never normalize or write it.
 *      Replay must go through `applyStreamPart` first and then flush; never
 *      build a second normalization pipeline in Rust.
 *
 * P0-2 invariant: after tee replay for an opened/settled session, the replay
 * batch is flushed durably exactly once, and the tee cursor advances only if
 * that flush succeeded. An empty canonical transcript is never trusted as
 * "already consumed" -- the replay starts from seq 0 instead.
 */
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
export type TranscriptPersistPlan =
  | { kind: "none" }
  | { kind: "append"; suffix: SessionEvent[] }
  | { kind: "rewrite"; events: SessionEvent[] }
  | { kind: "ensure-head" };

/**
 * Pure decision for the canonical transcript writer (kept DOM-free so tests
 * can pin the invariants):
 * - reference-identical prefix + growth -> append only the new suffix;
 * - same length -> nothing to do;
 * - a different rail with a truncated head -> load the head first (a rewrite
 *   must never eat history);
 * - otherwise rewrite the whole persistable rail.
 */
export function planTranscriptPersist(
  events: SessionEvent[],
  lastPersisted: SessionEvent[] | undefined,
  opts: { headTruncated: boolean },
): TranscriptPersistPlan {
  if (events.length === 0) return { kind: "none" };
  const prefixIdentical =
    lastPersisted != null &&
    events.length >= lastPersisted.length &&
    lastPersisted.every((ev, i) => events[i] === ev);
  if (lastPersisted != null && prefixIdentical) {
    if (events.length === lastPersisted.length) return { kind: "none" };
    return { kind: "append", suffix: events.slice(lastPersisted.length) };
  }
  if (opts.headTruncated) return { kind: "ensure-head" };
  return { kind: "rewrite", events };
}

/**
 * P0-2: an empty canonical transcript is never trusted as "already consumed".
 * The raw tee can always rebuild it, so replay starts at seq 0. Any stored
 * cursor from a previous (lost) flush is ignored.
 */
export function replayStartCursor(storedCursor: number, transcriptEmpty: boolean): number {
  if (transcriptEmpty) return 0;
  return Number.isFinite(storedCursor) && storedCursor > 0 ? storedCursor : 0;
}

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

  /**
   * X4（2026-11-06 内存压制批）：rail 自上次成功落盘后是否一字未变——与
   * `armTranscriptTimer` 的前缀比对同口径、同样**禁止克隆事件**（引用全等
   * 是判定本身，克隆会让它永远不成立——2026-10-31 的教训）。冷会话淘汰 /
   * 撕窗隐藏 flush 用它跳过 `persistRailsNow`（那会 ensureFullTranscript
   * 把截断头整段装回内存再全量重写文件，每 2 分钟一次的秒级卡顿源）：
   * 干净 = 磁盘本来就是最新，直接丢 rail 即可。
   * - rail 为空 → 干净（没有可写内容；真要写空表反而会清掉磁盘历史）。
   * - 从未落过盘（或上次写失败回滚了账本）→ 不干净，保守走全量。
   */
  const isPersistClean = (sessionId: string): boolean => {
    const events = persistableEventsForSession(
      shadowBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS,
      sessionId,
    );
    if (events.length === 0) return true;
    const last = lastPersistedRef.current.get(sessionId);
    if (last == null) return false;
    return events.length === last.length && last.every((ev, i) => events[i] === ev);
  };

  const persistChainsRef = useRef(new Map<string, Promise<boolean>>());

  /**
   * The single write routine for both callers -- the live debounce and the
   * explicit tee-replay flush. Returns true only when the rail's persistable
   * view is known to be on disk; a false answer must not advance the tee
   * cursor.
   */
  const persistSessionNow = async (
    sessionId: string,
    headRetry = false,
  ): Promise<boolean> => {
    if (!isTauriRuntime()) return false;
    if (!env.ownsAcpSession(sessionId)) return false;
    if (env.reloadingSessionsRef.current.has(sessionId)) return false;
    const pendingTimer = env.transcriptSaveTimers.current.get(sessionId);
    if (pendingTimer) {
      clearTimeout(pendingTimer);
      env.transcriptSaveTimers.current.delete(sessionId);
    }
    const events = persistableEventsForSession(
      shadowBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS,
      sessionId,
    );
    const plan = planTranscriptPersist(events, lastPersistedRef.current.get(sessionId), {
      headTruncated: env.isHeadTruncated(sessionId),
    });
    if (plan.kind === "none") return true;
    if (plan.kind === "ensure-head") {
      if (headRetry) return false;
      await env.ensureHeadForPersist(sessionId);
      if (env.isHeadTruncated(sessionId)) return false;
      return persistSessionNow(sessionId, true);
    }
    lastPersistedRef.current.set(sessionId, events);
    try {
      if (plan.kind === "append") {
        await appendTranscriptEvents(sessionId, plan.suffix);
      } else {
        await writeTranscript(sessionId, plan.events);
      }
      return true;
    } catch (error) {
      // Drop the accounting mark so the next flush conservatively falls back
      // to a full rewrite instead of appending a suffix that never landed.
      lastPersistedRef.current.delete(sessionId);
      env.pushDebugRef.current({
        sessionId,
        level: "warn",
        source: "transcript",
        summary:
          plan.kind === "append"
            ? "append transcript failed; falling back to full write"
            : "write transcript failed",
        detail: error instanceof Error ? error.message : String(error),
      });
      return false;
    }
  };

  /** Serialize writes per session so the debounce and replay cannot double-write. */
  const enqueuePersist = (sessionId: string): Promise<boolean> => {
    const previous = persistChainsRef.current.get(sessionId) ?? Promise.resolve(true);
    const run = previous.catch(() => false).then(() => persistSessionNow(sessionId));
    persistChainsRef.current.set(sessionId, run);
    void run
      .finally(() => {
        if (persistChainsRef.current.get(sessionId) === run) {
          persistChainsRef.current.delete(sessionId);
        }
      })
      .catch(() => undefined);
    return run;
  };

  /**
   * Explicit flush used by tee replay before its cursor advances. It cancels
   * the pending debounce and writes through the same plan as the live path.
   */
  const flushSessionTranscript = (sessionId: string): Promise<boolean> =>
    enqueuePersist(sessionId);

  const armTranscriptTimer = (sessionId: string) => {
    if (!isTauriRuntime()) return;
    if (!env.ownsAcpSession(sessionId)) return;
    if (env.reloadingSessionsRef.current.has(sessionId)) return;
    const prev = env.transcriptSaveTimers.current.get(sessionId);
    if (prev) clearTimeout(prev);
    const sizeHint = shadowBySessionRef.current.get(sessionId)?.length ?? 0;
    const delay = sizeHint > FLUSH_DEBOUNCE_SLOW_EVENTS ? FLUSH_DEBOUNCE_SLOW_MS : FLUSH_DEBOUNCE_MS;
    const timer = setTimeout(() => {
      env.transcriptSaveTimers.current.delete(sessionId);
      void enqueuePersist(sessionId);
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
    isPersistClean,
    flushSessionTranscript,
  };
}
