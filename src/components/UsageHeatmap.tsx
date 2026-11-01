import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent as ReactFocusEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { useLocale, useT } from "../lib/i18n";
import { formatTokenCount } from "../lib/sessionStats";
import { dayKeyOf } from "../lib/usageLedger";
import {
  buildHeatModel,
  heatGridPos,
  heatWeekdayLabels,
  type HeatDay,
} from "../lib/activityHeatmap";

/**
 * UsageHeatmap — Token 用量分析里的「每日活动格子」(GitHub 式)。
 *
 * 骨架学 uiarc ActivityHeatmap(2026-11-01 产品点名),按仓库约束重写:
 * 零依赖(不用 motion 库)、动效只 transition transform/opacity、走
 * motion.css 的时长 token;保留它真正值钱的交互——
 *
 * - 键盘漫游焦点(↑↓ 一天、←→ 一周、Home/End、Enter/Space 选中、Esc 收起),
 *   网格 role=grid/row/gridcell,单元格里只有一个 tab stop;
 * - 全网格**共享一个 tooltip**:跳开滑移(120ms transform),悬停离开 110ms
 *   收、触屏点按留 2400ms、键盘用户聚焦期间不收;
 * - 图例=等级过滤器:hover/焦点预览、点击钉住(aria-pressed),不匹配的
 *   日子变淡,说明行走 aria-live polite;
 * - 选中环是**一个**绝对定位元素,transform 滑移到选中日(uiarc 同款,
 *   不给每个格子画 outline);
 * - 月标残月让位(隔 <3 列丢首标)、星期标只标一/三/五。
 */

const CELL = 11;
const GAP = 2;
const PITCH = CELL + GAP;
const HIDE_MS = 110;
const TOUCH_LINGER_MS = 2400;

export type UsageHeatDay = HeatDay & { turns?: number };

type Tip = { index: number; primary: string; secondary: string } | null;

export function UsageHeatmap({ days }: { days: UsageHeatDay[] }) {
  const t = useT();
  const locale = useLocale();
  const bcp47 = locale === "en" ? "en" : "zh-CN";

  const model = useMemo(
    () => buildHeatModel(days, { weekStartsOn: 1, locale: bcp47 }),
    [days, bcp47],
  );
  const weekdayLabels = useMemo(() => heatWeekdayLabels(1, bcp47), [bcp47]);
  const formats = useMemo(
    () => ({
      long: new Intl.DateTimeFormat(bcp47, {
        weekday: "long",
        month: "long",
        day: "numeric",
        year: "numeric",
      }),
      short: new Intl.DateTimeFormat(bcp47, {
        weekday: "short",
        month: "short",
        day: "numeric",
      }),
    }),
    [bcp47],
  );
  /** 稠密 turns(与 model.counts 同下标),tooltip 与 aria 都要说轮数。 */
  const turnsByIdx = useMemo(() => {
    const out = new Array<number>(model.length).fill(0);
    for (const day of days) {
      const time = new Date(`${day.key}T00:00:00`).getTime();
      if (!Number.isFinite(time)) continue;
      const index = Math.round((time - model.start) / 86_400_000);
      if (index >= 0 && index < model.length) out[index] = day.turns ?? 0;
    }
    return out;
  }, [days, model]);

  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [focusIndex, setFocusIndex] = useState<number | null>(null);
  const [preview, setPreview] = useState<number | null>(null);
  const [pinned, setPinned] = useState<number | null>(null);
  const highlight = preview ?? pinned;
  const [tip, setTip] = useState<Tip>(null);
  const [tipOpen, setTipOpen] = useState(false);

  const gridRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<HTMLDivElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const hideTimer = useRef(0);

  useEffect(() => () => window.clearTimeout(hideTimer.current), []);

  // 窄面板横向滚动,默认锚到最新一周(右侧)。
  useEffect(() => {
    const scroller = scrollerRef.current;
    if (scroller) scroller.scrollLeft = scroller.scrollWidth;
  }, [model.weeks]);

  const keyOf = (index: number): string =>
    dayKeyOf(new Date(model.start + index * 86_400_000));

  const contentFor = (index: number): NonNullable<Tip> => {
    const count = model.counts[index] ?? 0;
    const turns = turnsByIdx[index] ?? 0;
    const value = formatTokenCount(count);
    return {
      index,
      primary:
        count > 0
          ? `${value} · ${t("usageLedger.turns", { count: turns })}`
          : t("usageLedger.heatNone"),
      secondary: formats.short.format(new Date(model.start + index * 86_400_000)),
    };
  };

  function showTip(next: NonNullable<Tip>) {
    window.clearTimeout(hideTimer.current);
    setTip(next);
    setTipOpen(true);
  }

  function hideTipSoon(delay = HIDE_MS) {
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => setTipOpen(false), delay);
  }

  // tooltip 位置:元素常驻、透明度开关;开着的滑移由 CSS transition 接管,
  // 关着挪位看不见,重开自然落在新格子(uiarc 的 jump-on-open 免状态版)。
  const [tipPos, setTipPos] = useState<{ x: number; y: number } | null>(null);
  useLayoutEffect(() => {
    if (!tip) return;
    const cell = gridRef.current?.querySelector<HTMLElement>(`[data-index="${tip.index}"]`);
    if (!cell) return;
    const place = () =>
      setTipPos({ x: cell.offsetLeft + cell.offsetWidth / 2, y: cell.offsetTop });
    place();
    // 横向滚动时钉在格子上。
    const scroller = scrollerRef.current;
    scroller?.addEventListener("scroll", place, { passive: true });
    return () => scroller?.removeEventListener("scroll", place);
  }, [tip]);

  const indexOfElement = (element: Element | null): { cell: HTMLElement; index: number } | null => {
    const cell = element?.closest<HTMLElement>("[data-index]");
    return cell && gridRef.current?.contains(cell)
      ? { cell, index: Number(cell.dataset.index) }
      : null;
  };

  const focusDay = (index: number) => {
    gridRef.current
      ?.querySelector<HTMLElement>(`[data-index="${Math.max(0, Math.min(index, model.length - 1))}"]`)
      ?.focus();
  };

  function onGridPointerOver(event: ReactPointerEvent<HTMLDivElement>) {
    const hit = indexOfElement(event.target as Element);
    if (hit) showTip(contentFor(hit.index));
  }

  function onGridPointerLeave(event: ReactPointerEvent<HTMLDivElement>) {
    // 键盘用户的 tooltip 钉在聚焦日上;触屏点按后多留一会儿。
    const focused = indexOfElement(document.activeElement);
    if (focused?.cell.matches(":focus-visible")) showTip(contentFor(focused.index));
    else hideTipSoon(event.pointerType === "touch" ? TOUCH_LINGER_MS : HIDE_MS);
  }

  function onGridFocus(event: ReactFocusEvent<HTMLDivElement>) {
    const hit = indexOfElement(event.target);
    if (!hit) return;
    setFocusIndex(hit.index);
    showTip(contentFor(hit.index));
  }

  function onGridBlur(event: ReactFocusEvent<HTMLDivElement>) {
    if (!gridRef.current?.contains(event.relatedTarget as Node | null)) hideTipSoon(0);
  }

  function onGridKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const hit = indexOfElement(event.target as Element);
    if (!hit) return;
    const moves: Record<string, number> = {
      ArrowUp: hit.index - 1,
      ArrowDown: hit.index + 1,
      ArrowLeft: hit.index - 7,
      ArrowRight: hit.index + 7,
      Home: 0,
      End: model.length - 1,
    };
    if (event.key in moves) {
      event.preventDefault();
      focusDay(moves[event.key]!);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      setSelectedKey(keyOf(hit.index));
    } else if (event.key === "Escape" && tipOpen) {
      event.preventDefault();
      event.stopPropagation();
      setTipOpen(false);
    }
  }

  const selectedIndex = selectedKey
    ? Math.round((new Date(`${selectedKey}T00:00:00`).getTime() - model.start) / 86_400_000)
    : -1;
  const hasSelection = selectedIndex >= 0 && selectedIndex < model.length;
  const tabIndexDay =
    focusIndex != null && focusIndex < model.length
      ? focusIndex
      : hasSelection
        ? selectedIndex
        : model.length - 1;
  const selectedPos = hasSelection ? heatGridPos(selectedIndex, model.lead) : null;

  const totalTurns = turnsByIdx.reduce((a, b) => a + b, 0);
  const [a, b, c] = model.thresholds;
  const ranges = [
    t("usageLedger.heatNone"),
    `1–${formatTokenCount(a)}`,
    `${formatTokenCount(a + 1)}–${formatTokenCount(b)}`,
    `${formatTokenCount(b + 1)}–${formatTokenCount(c)}`,
    `${formatTokenCount(c + 1)}+`,
  ];
  const caption =
    highlight == null
      ? ""
      : t("usageLedger.heatDays", {
          count: model.perLevel[highlight] ?? 0,
          range: ranges[highlight] ?? "",
        });

  if (model.length === 0) return null;

  return (
    <div className="usage-heat">
      <p className="usage-heat__summary">
        <span className="usage-heat__total">
          {t("usageLedger.heatSummary", {
            weeks: model.weeks,
            tokens: formatTokenCount(model.total),
            count: totalTurns,
          })}
        </span>
        <span className="usage-updated" role="status">
          {hasSelection ? contentFor(selectedIndex).primary : ""}
        </span>
      </p>

      <div ref={scrollerRef} className="usage-heat__scroller">
        <div className="usage-heat__canvas" style={{ width: model.weeks * PITCH }}>
          <div className="usage-heat__months" aria-hidden>
            {model.months.map((month) => (
              <span
                key={month.monthIndex}
                className="usage-heat__month"
                style={{ transform: `translateX(${month.col * PITCH}px)` }}
              >
                {month.label}
              </span>
            ))}
          </div>
          <div className="usage-heat__row usage-heat__row--labels" aria-hidden>
            {weekdayLabels.map((label, row) => (
              <span key={row} className="usage-heat__weekday">
                {label}
              </span>
            ))}
          </div>
          <div ref={plotRef} className="usage-heat__plot">
            <div
              ref={gridRef}
              className="usage-heat__grid"
              role="grid"
              aria-label={t("usageLedger.dailyTitle")}
              aria-readonly="true"
              data-highlight={highlight ?? undefined}
              onPointerOver={onGridPointerOver}
              onPointerLeave={onGridPointerLeave}
              onFocus={onGridFocus}
              onBlur={onGridBlur}
              onKeyDown={onGridKeyDown}
            >
              {Array.from({ length: 7 }, (_, row) => (
                <div key={row} role="row" className="usage-heat__row">
                  {Array.from({ length: model.weeks }, (_, col) => {
                    const index = col * 7 + row - model.lead;
                    if (index < 0 || index >= model.length) {
                      return <span key={col} className="usage-heat__cell" data-empty aria-hidden />;
                    }
                    const level = model.levels[index] ?? 0;
                    const date = new Date(model.start + index * 86_400_000);
                    return (
                      <span
                        key={col}
                        role="gridcell"
                        className="usage-heat__cell"
                        data-index={index}
                        data-lv={level}
                        tabIndex={index === tabIndexDay ? 0 : -1}
                        aria-selected={hasSelection && index === selectedIndex}
                        aria-label={`${contentFor(index).primary} · ${formats.long.format(date)}`}
                        onClick={() => setSelectedKey(keyOf(index))}
                      />
                    );
                  })}
                </div>
              ))}
            </div>
            {selectedPos && (
              <span
                className="usage-heat__ring"
                aria-hidden
                style={{
                  transform: `translate(${selectedPos.col * PITCH}px, ${selectedPos.row * PITCH}px)`,
                }}
              />
            )}
            {tip && tipPos && (
              <span
                className="usage-heat__tip"
                aria-hidden
                data-open={tipOpen || undefined}
                style={{ transform: `translate(${tipPos.x}px, ${tipPos.y}px)` }}
              >
                <span className="usage-heat__tip-primary">{tip.primary}</span>
                <span className="usage-heat__tip-secondary">{tip.secondary}</span>
              </span>
            )}
          </div>
        </div>
      </div>

      <div className="usage-heat__legend">
        <span className="usage-heat__sr">
          {t("usageLedger.heatLegendSr", { ranges: ranges.join(", ") })}
        </span>
        <span className="usage-heat__caption" aria-live="polite">
          {caption}
        </span>
        <span className="usage-heat__scale">
          <span className="usage-heat__legend-text" aria-hidden>
            {t("usageLedger.heatLess")}
          </span>
          {[0, 1, 2, 3, 4].map((level) => (
            <button
              key={level}
              type="button"
              className="usage-heat__swatch"
              data-lv={level}
              aria-pressed={pinned === level}
              aria-label={t("usageLedger.heatHighlight", { range: ranges[level] ?? "" })}
              onClick={() => setPinned((current) => (current === level ? null : level))}
              onPointerEnter={() => setPreview(level)}
              onPointerLeave={() => setPreview(null)}
              onFocus={() => setPreview(level)}
              onBlur={() => setPreview(null)}
            />
          ))}
          <span className="usage-heat__legend-text" aria-hidden>
            {t("usageLedger.heatMore")}
          </span>
        </span>
      </div>
    </div>
  );
}
