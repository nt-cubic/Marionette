/**
 * attention 统一投影（第六期「安静」，学 orca 的 fleet attention）：
 * 把一个任务/会话的事实（状态、等待原因、重试排程、最后活动时间）归约成
 * 一个注意力类别 +「现在需要人吗」。设计目标不是让人知道更多，而是
 * **只在真正需要人的时刻打扰**：
 *
 * - requiresAction = true 的类别（approval / input / failure / interruption /
 *   stalled）才有资格响铃/置顶/推 Telegram；
 * - waiting_retry（额度午休）明确「不需要你」——它在自己等；
 * - settled 永远安静。
 *
 * 纯函数、无 IO；后台任务面板是第一个消费方，桌面通知 / Telegram 接入
 * 同一真相（后续批次）。
 */

export type AttentionCategory =
  /** 在等用户批准（权限卡 / 计划批准）。 */
  | "approval"
  /** 在等用户回答（提问 / 补充输入）。 */
  | "input"
  /** 任务失败，需要人看一眼。 */
  | "failure"
  /** 任务被中断（重启 / 失联），可 resume。 */
  | "interruption"
  /** 活着但长时间无任何动静（卡死）。 */
  | "stalled"
  /** 额度/限流受限，正在排队自动重试——不需要人。 */
  | "waiting_retry"
  /** 正常在跑。 */
  | "running"
  /** 已结束（完成/取消），安静。 */
  | "settled";

export type AttentionFact = {
  /** 任务/会话状态（chief 任务状态机口径）。 */
  status:
    | "proposed"
    | "starting"
    | "running"
    | "waiting_user"
    | "completed"
    | "failed"
    | "cancelled"
    | "interrupted";
  /** waiting_user 时的原因（permission/question/plan_approval/user_input/auto_retry）。 */
  waitingReason?: string | null;
  /** 自动重试的到点时间（Unix 毫秒）。 */
  retryAt?: number | null;
  /** 最后一次活动（Unix 毫秒）；用于卡死判定。 */
  lastEventAt?: number | null;
  /** 当前时间（默认 Date.now()；测试注入用）。 */
  now?: number;
};

export type Attention = {
  category: AttentionCategory;
  /** 现在需要人吗——true 的类别才有资格打扰。 */
  requiresAction: boolean;
};

/** 卡死线：与 Rust 侧 WORKER_STALL_MS 同口径（20 分钟）。 */
export const STALL_MS = 20 * 60 * 1000;

export function projectAttention(fact: AttentionFact): Attention {
  const now = fact.now ?? Date.now();
  switch (fact.status) {
    case "waiting_user": {
      if (fact.waitingReason === "permission" || fact.waitingReason === "plan_approval") {
        return { category: "approval", requiresAction: true };
      }
      return { category: "input", requiresAction: true };
    }
    case "failed":
      return { category: "failure", requiresAction: true };
    case "interrupted":
      return { category: "interruption", requiresAction: true };
    case "completed":
    case "cancelled":
      return { category: "settled", requiresAction: false };
    case "running":
    case "starting":
    case "proposed":
    default: {
      // 额度午休优先于卡死判定——它在排队，不在干活也不算失踪。
      if (fact.waitingReason === "auto_retry" && fact.retryAt != null) {
        return { category: "waiting_retry", requiresAction: false };
      }
      if (
        typeof fact.lastEventAt === "number" &&
        fact.lastEventAt > 0 &&
        now - fact.lastEventAt > STALL_MS
      ) {
        return { category: "stalled", requiresAction: true };
      }
      return { category: "running", requiresAction: false };
    }
  }
}

/** 类别的人话标签键（i18n；消费方用它渲染，不再各自拼状态词）。 */
export const ATTENTION_LABEL_KEY: Record<AttentionCategory, string> = {
  approval: "attention.approval",
  input: "attention.input",
  failure: "attention.failure",
  interruption: "attention.interruption",
  stalled: "attention.stalled",
  waiting_retry: "attention.waitingRetry",
  running: "attention.running",
  settled: "attention.settled",
};
