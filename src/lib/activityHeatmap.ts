/**
 * activityHeatmap — GitHub 式活动格子的纯数据模型(移植自 uiarc
 * ActivityHeatmap 的 buildModel,按 Marionette 口径改造):
 *
 * - 输入是稀疏的 { key: YYYY-MM-DD, value } 日账,内部摊平成稠密
 *   counts[](缺日为 0),lead 把首周补齐到 weekStartsOn 的行位。
 * - 等级阈值缺省取最忙日的 1/4、1/2、3/4(向上取整,至少 1/2/3),
 *   与 codeg/uiarc 同口径;value<=0 恒为 0 级。
 * - 月标(uiarc 规则):首月是残月时,只有当它与下一个月标之间隔 ≥3 列
 *   才保留,否则丢掉(否则开头挤两个标签)。
 * - 日期一律**本地钟**(台账 key 就是本地日),不做 UTC 折叠。
 *
 * UI(UsageHeatmap.tsx)只管交互:键盘漫游、共享 tooltip、图例过滤。
 * 回归:scripts/verify-usage-ledger.mts。
 */

export type HeatDay = { key: string; value: number };

export type HeatMonthMark = { monthIndex: number; col: number; label: string };

export type HeatModel = {
  /** 首日 key(输入里最早的一天)。 */
  startKey: string;
  start: number;
  /** 覆盖天数(含首尾)。 */
  length: number;
  /** 首周在 weekStartsOn 行序下的空位數。 */
  lead: number;
  weeks: number;
  total: number;
  counts: number[];
  levels: number[];
  thresholds: [number, number, number];
  months: HeatMonthMark[];
  /** 每个等级的天数(0..4)。 */
  perLevel: number[];
};

const DAY = 86_400_000;

const toTime = (key: string): number => {
  const t = new Date(`${key}T00:00:00`).getTime();
  return Number.isFinite(t) ? t : Number.NaN;
};

/** 本地 YYYY-MM-DD(别用 toISOString——UTC 折叠会把东八区的本地零点推早一天)。 */
const toKey = (time: number): string => {
  const d = new Date(time);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

export const heatDayCount = DAY;

/** 稀疏日账 → 稠密模型。days 乱序也行;越界 value 按 0 处理。 */
export function buildHeatModel(
  days: HeatDay[],
  options: { weekStartsOn?: 0 | 1; thresholds?: [number, number, number]; locale?: string } = {},
): HeatModel {
  const weekStartsOn = options.weekStartsOn ?? 1;
  const valid = days
    .map((day) => ({ time: toTime(day.key), value: Math.max(0, day.value) }))
    .filter((day) => Number.isFinite(day.time));
  const start = valid.length ? Math.min(...valid.map((d) => d.time)) : toTime("2026-01-01");
  const length = valid.length
    ? Math.round((Math.max(...valid.map((d) => d.time)) - start) / DAY) + 1
    : 0;
  const counts = new Array<number>(length).fill(0);
  for (const day of valid) {
    const index = Math.round((day.time - start) / DAY);
    if (index >= 0 && index < length) counts[index] = counts[index]! + day.value;
  }
  const max = counts.reduce((a, b) => Math.max(a, b), 0);
  const bounds: [number, number, number] =
    options.thresholds ?? [
      Math.max(1, Math.ceil(max * 0.25)),
      Math.max(2, Math.ceil(max * 0.5)),
      Math.max(3, Math.ceil(max * 0.75)),
    ];
  const levels = counts.map((count) =>
    count <= 0 ? 0 : count <= bounds[0] ? 1 : count <= bounds[1] ? 2 : count <= bounds[2] ? 3 : 4,
  );
  const lead = (new Date(start).getDay() - weekStartsOn + 7) % 7;
  const weeks = Math.ceil((lead + length) / 7);
  const monthName = new Intl.DateTimeFormat(options.locale ?? "default", { month: "short" });
  const months: HeatMonthMark[] = [];
  for (let index = 0; index < length; index += 1) {
    const date = new Date(start + index * DAY);
    if (index === 0 || date.getDate() === 1) {
      months.push({
        monthIndex: date.getFullYear() * 12 + date.getMonth(),
        col: Math.floor((lead + index) / 7),
        label: monthName.format(date),
      });
    }
  }
  // 残月标签只有与下一个月标隔 ≥3 列才保留(uiarc 同规则)。
  if (months.length > 1 && months[1]!.col - months[0]!.col < 3) months.shift();
  const perLevel = [0, 0, 0, 0, 0];
  for (const level of levels) perLevel[level] = perLevel[level]! + 1;
  return {
    startKey: toKey(start),
    start,
    length,
    lead,
    weeks,
    total: counts.reduce((a, b) => a + b, 0),
    counts,
    levels,
    thresholds: bounds,
    months,
    perLevel,
  };
}

/**
 * 左侧星期标:uiarc 的稀疏法——只标奇数星期(weekStartsOn=1 时即
 * 一/三/五),偶数行留空。1970-01-04 是周日,`(3 + weekday) * DAY`
 * 取任意星期的锚日期。
 */
export function heatWeekdayLabels(weekStartsOn: 0 | 1 = 1, locale = "default"): string[] {
  const fmt = new Intl.DateTimeFormat(locale, { weekday: "short" });
  return Array.from({ length: 7 }, (_, row) => {
    const weekday = (row + weekStartsOn) % 7;
    return weekday % 2 === 1 ? fmt.format(new Date((3 + weekday) * DAY)) : "";
  });
}

/** index → 网格位(列 = 周,行 = 星期);供选中环与键盘移动共用。 */
export function heatGridPos(index: number, lead: number): { col: number; row: number } {
  const cell = index + lead;
  return { col: Math.floor(cell / 7), row: cell % 7 };
}
