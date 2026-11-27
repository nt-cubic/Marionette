/**
 * Chief durable event tee 的回放辅助（docs/research/telegram-chief/）。
 *
 * 被 Chief/worker 后端驱动的会话不经过前端 rails 落盘，事件由 Rust 侧
 * tee 原样记录（`~/.marionette/chief/sessions/<id>.events.jsonl`）。会话
 * 在本地打开时，App 把 tee 事件喂进与活路**完全相同**的转换管线
 * （`extractAcpUpdateText` + `rails.applyStreamPart`），保证渲染一致；
 * 这里只放纯逻辑：游标过滤、正文合并、收尾。
 */

/** tee 文件一行（Rust 侧 {seq, ts, sessionId, kind, method, data}）。 */
export interface ChiefTeeEvent {
  seq: number;
  ts?: number;
  sessionId?: string;
  kind?: string;
  method?: string | null;
  data?: unknown;
}

/** 只回放游标之后的事件（seq 严格递增；坏行已被 Rust 侧跳过）。 */
export function teeEventsAfterCursor(
  events: ReadonlyArray<ChiefTeeEvent>,
  cursor: number,
): ChiefTeeEvent[] {
  return events.filter(
    (event) => typeof event?.seq === "number" && Number.isFinite(event.seq) && event.seq > cursor,
  );
}

/** 本批最大 seq（consume 游标推进用；空批返回 floor）。 */
export function maxSeqOf(events: ReadonlyArray<ChiefTeeEvent>, floor: number): number {
  let max = floor;
  for (const event of events) {
    if (typeof event?.seq === "number" && Number.isFinite(event.seq) && event.seq > max) {
      max = event.seq;
    }
  }
  return max;
}

/**
 * 未覆盖跨度过滤：tee 事件里 ts（发生时刻毫秒）**晚于**转录最新事件
 * createdAt 的部分才需要回填。转录是追加式、卡片的 createdAt 在转换时
 * 打点（≥ 事件发生时刻），所以「ts ≤ 转录最新 createdAt」的事件必然已经
 * 在转录里（活路或上一轮回放写入过）——按 ts 过滤让「UI 正常用过的
 * Chief 会话」不会把已有历史重放一遍（游标只覆盖后端回填自己消费过的
 * 跨度，盖不住 UI 自己写的部分）。缺 ts 的事件保守保留（宁重放不丢）。
 */
export function uncoveredTeeEvents(
  events: ReadonlyArray<ChiefTeeEvent>,
  transcriptMaxMs: number,
): ChiefTeeEvent[] {
  return events.filter(
    (event) => typeof event?.ts !== "number" || !Number.isFinite(event.ts) || event.ts > transcriptMaxMs,
  );
}

/** 转录事件里最新的 createdAt（毫秒；解析不出 = 0）。 */
export function transcriptMaxCreatedAtMs(
  events: ReadonlyArray<{ createdAt?: string }>,
): number {
  let max = 0;
  for (const event of events) {
    const ms = event?.createdAt ? Date.parse(event.createdAt) : Number.NaN;
    if (Number.isFinite(ms) && ms > max) max = ms;
  }
  return max;
}

/** 是否后端驱动家族来源（chief 会话本体 / 各类 worker）。挂起豁免、工作集
 * 压制豁免、tee 回填都按这个家族判定——agent-worker（AI 派活的子会话）
 * 与 chief-worker 同为后端 broker 驱动，待遇一致。 */
export function isChiefOrigin(origin?: string | null): boolean {
  return origin === "chief" || origin === "chief-worker" || origin === "agent-worker";
}

/**
 * user echo 正文合并：ACP 的 user_message_chunk 可能是增量或累计快照
 * （与 agent chunk 同语义），用与活路同一条合并规则。
 */
export function mergeUserEcho(
  previous: string,
  incoming: { text: string; isDelta?: boolean },
): string {
  if (!incoming.text) return previous;
  if (incoming.isDelta) return previous + incoming.text;
  // 累计快照：取更长的一方（退化保护，正常应 incoming 覆盖）。
  return incoming.text.length >= previous.length ? incoming.text : previous;
}

/**
 * user echo 的 flush 时机：第一条非 user 输出（agent/thought/tool/system/
 * other agent 活动）之前，或批次结束时。方法级判定。
 */
export function shouldFlushUserEcho(method: string | null | undefined, kind?: string): boolean {
  if (method === "turn/complete" || method === "process/ended" || method === "process/stopped") {
    return true;
  }
  // agent 开始说话/动手 = 前一段 user 输入已完整。
  return kind !== undefined && kind !== "" && kind !== "user_message_chunk";
}

/**
 * P0-3: durable tee health, as returned by `chief_tee_health`.
 *
 * Counts only. `dropped` = events the bounded queue refused before they ever
 * reached disk (unrecoverable); `fileLines` = records that did reach disk;
 * `consumedSeq` = how far replay has been applied and flushed.
 */
export interface ChiefTeeHealth {
  sessionId: string;
  fileLines: number;
  enqueued: number;
  written: number;
  dropped: number;
  badLines: number;
  consumedSeq: number;
  replayStartSeq: number;
  replayEvents: number;
  replayTruncated: boolean;
  registered: boolean;
  updatedAt: number;
}

/** One recovery read. Keep small so a retry cannot balloon memory. */
export const TEE_RETRY_SEGMENT = 400;

export type TeeNoticeState = {
  visible: boolean;
  lost: number;
  kept: number;
  remaining: number;
  badLines: number;
  /** There is durable ledger content that a retry can re-read. */
  canRetry: boolean;
};

function finiteCount(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? Math.floor(value)
    : 0;
}

/** Pure state fold for the worker notice (no copy, no React). */
export function teeNoticeState(
  health: ChiefTeeHealth | null | undefined,
): TeeNoticeState {
  const lost = finiteCount(health?.dropped);
  const kept = finiteCount(health?.fileLines);
  const consumed = finiteCount(health?.consumedSeq);
  return {
    visible: lost > 0,
    lost,
    kept,
    remaining: Math.max(0, kept - consumed),
    badLines: finiteCount(health?.badLines),
    canRetry: kept > 0,
  };
}

/**
 * P0-3 recovery start. A torn/empty canonical transcript is rebuilt from the
 * ledger's first record; otherwise only records past the flushed cursor are
 * re-read, so repeated retries cannot re-insert cards already on disk.
 */
export function teeRetryStartCursor(consumedSeq: number, railLen: number): number {
  if (railLen <= 0) return 0;
  return finiteCount(consumedSeq);
}

export type TeeNoticeCopy = {
  titleKey: string;
  titleParams: Record<string, string | number>;
  bodyKey: string;
  bodyParams: Record<string, string | number>;
  remainingKey: string;
  remainingParams: Record<string, string | number>;
  badLinesKey: string | null;
  badLinesParams: Record<string, string | number>;
  retryKey: string;
  dismissKey: string;
};

/**
 * Pure copy descriptor (keys + params). The component resolves it through `t`,
 * so locale/plural rules stay in the existing i18n layer.
 */
export function teeNoticeCopy(state: TeeNoticeState): TeeNoticeCopy {
  return {
    titleKey: "session.teeLossTitle",
    titleParams: { lost: state.lost },
    bodyKey: "session.teeLossBody",
    bodyParams: { lost: state.lost, kept: state.kept },
    remainingKey:
      state.remaining > 0 ? "session.teeLossRemaining" : "session.teeLossConsumed",
    remainingParams: { remaining: state.remaining, kept: state.kept },
    badLinesKey: state.badLines > 0 ? "session.teeLossBadLines" : null,
    badLinesParams: { bad: state.badLines },
    retryKey: state.canRetry ? "session.teeLossRetry" : "session.teeLossNoRetry",
    dismissKey: "session.teeLossDismiss",
  };
}
