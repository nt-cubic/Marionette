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

/** 是否 Chief 家族来源（chief 会话本体 / worker）。 */
export function isChiefOrigin(origin?: string | null): boolean {
  return origin === "chief" || origin === "chief-worker";
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
