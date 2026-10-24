/**
 * The docked strip's chrome (edge dock, docs/plan-edge-dock.md §3; layout per
 * docs/design/edge-dock.html ②).
 *
 * One fixed band above the conversation, top to bottom:
 *   title bar   brand · search · usage · ⋯   — or ‹ 返回 + page title on a page
 *   actions     新建对话 · 收纳盒
 *   quota       one compact meter per subscription + balance chips, tap = usage page
 *   active      dialogs still holding a process + the one in view; replaces tabs
 * Pages (all dialogs / usage / memo box) cover everything below the title bar
 * and come back with ‹ 返回 or Esc. Undock / update / quit live in ⋯. The
 * band's height is measured into --dock-chrome-top for the app-shell padding.
 */

import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Archive, ChevronLeft, MoreHorizontal, Pause, Plus, Search } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { EngineQuotaEntry, EngineQuotaWindow } from "../lib/api";
import { lastActiveMs } from "../lib/sessionSuspend";
import { beginDockBarDrag } from "../lib/dockMode";
import type { Session } from "../lib/types";

export type DockPage = "shelf" | "context" | "memo";

const PAGE_TITLES: Record<DockPage, string> = {
  shelf: "全部对话",
  context: "用量与项目",
  memo: "收纳盒",
};

type Props = {
  page: DockPage | null;
  onPage: (page: DockPage | null) => void;
  onNewChat: () => void;
  memoCount: number;
  /** Subscription families from `magpie quota`; every one gets a ring. */
  quotaRows: EngineQuotaEntry[];
  /** Dialogs still holding a process, plus the one in view; newest first. */
  active: Session[];
  totalSessions: number;
  currentSessionId: string | null;
  doneUnreadIds: ReadonlySet<string>;
  onOpenSession: (session: Session) => void;
  onSuspendSession: (sessionId: string) => void;
  onOpenPalette: () => void;
  onUndock: () => void;
  onQuit: () => void;
  updateAvailable: boolean;
  updateLabel: string | null;
  updateBusy: boolean;
  onCheckUpdate: () => void;
  onUpdateAndRelaunch: () => void;
  /** A split workspace is paused while docked. */
  splitPaused: boolean;
};

function isBusy(session: Session): boolean {
  return session.status === "running" || session.status === "starting";
}

function isAlive(session: Session): boolean {
  return isBusy(session) || session.status === "waiting" || session.status === "error";
}

function recency(session: Session, nowMs: number): string {
  const ms = lastActiveMs(session);
  if (!ms) return "";
  const min = Math.floor((nowMs - ms) / 60_000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min}m`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h`;
  return `${Math.floor(hr / 24)}d`;
}

/** `Grok (SuperGrok)` → `Grok`: the plan name is on the usage page. */
function shortName(name: string): string {
  return name.replace(/\s*\([^()]*\)\s*$/, "").trim() || name;
}

function usedOf(window: EngineQuotaWindow): number | null {
  if (typeof window.used === "number") return Math.max(0, Math.min(100, window.used));
  if (typeof window.remaining === "number") return Math.max(0, Math.min(100, 100 - window.remaining));
  return null;
}

function tone(used: number | null): string {
  if (used == null) return "is-none";
  return used >= 90 ? "is-hot" : used >= 75 ? "is-warm" : "is-ok";
}

function resetLabel(iso?: string | null): string {
  if (!iso) return "";
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return "";
  const left = ms - Date.now();
  if (left <= 0) return "";
  const hr = Math.floor(left / 3_600_000);
  if (hr < 1) return ` · ${Math.max(1, Math.round(left / 60_000))} 分钟后重置`;
  if (hr < 48) return ` · ${hr} 小时后重置`;
  return ` · ${Math.round(hr / 24)} 天后重置`;
}

/** Hover text is the full truth: every window + reset time, then the balance. */
function quotaTitle(row: EngineQuotaEntry): string {
  return [
    row.name,
    ...row.windows.map((w) => {
      const used = usedOf(w);
      return `${w.name}  ${used == null ? "—" : `${Math.round(used)}%`}${resetLabel(w.resetsAt)}`;
    }),
    ...(row.balance ? [`余额 ${row.balance}`] : []),
  ].join("\n");
}

function windowsOf(row: EngineQuotaEntry): Array<{ w: EngineQuotaWindow; used: number }> {
  return row.windows
    .map((w) => ({ w, used: usedOf(w) }))
    .filter((x): x is { w: EngineQuotaWindow; used: number } => x.used != null)
    .sort((a, b) => b.used - a.used);
}

/**
 * One source as a mini card — name, the tightest window's percentage, and a
 * single 3px bar. QuotaBar's rule: the headline is one number, the windows
 * live in the hover/usage page; nesting two arcs in 40px read as noise.
 */
function QuotaMeter({ row, onOpen }: { row: EngineQuotaEntry; onOpen: () => void }) {
  const peak = windowsOf(row)[0]?.used ?? null;
  const name = shortName(row.name);
  return (
    <button
      type="button"
      className={`dock-meter ${tone(peak)}`}
      title={quotaTitle(row)}
      aria-label={`${name} ${peak == null ? "暂无额度数据" : `${Math.round(peak)}%`} · 点开看明细`}
      onClick={onOpen}
    >
      <span className="dock-meter__name">{name}</span>
      <span className="dock-meter__pct">{peak == null ? "—" : `${Math.round(peak)}%`}</span>
      <span className="dock-meter__bar" aria-hidden>
        <i style={{ width: `${peak ?? 0}%` }} />
      </span>
    </button>
  );
}

function QuotaMeters({ rows, onOpen }: { rows: EngineQuotaEntry[]; onOpen: () => void }) {
  const sorted = useMemo(() => {
    const peak = (row: EngineQuotaEntry) => windowsOf(row)[0]?.used ?? -1;
    return [...rows].sort((a, b) => peak(b) - peak(a));
  }, [rows]);
  if (sorted.length === 0) return null;
  // A balance-only source gets a chip on its own line (QuotaBar shows the
  // amount, not an empty 0% bar); sources with windows — or with neither —
  // get a meter card.
  const balances = sorted.filter((row) => row.balance && windowsOf(row).length === 0);
  const meters = sorted.filter((row) => !balances.includes(row));
  return (
    <div className="dock-quota">
      {meters.length > 0 && (
        <div className="dock-meters">
          {meters.map((row) => (
            <QuotaMeter key={`${row.provider}:${row.name}`} row={row} onOpen={onOpen} />
          ))}
        </div>
      )}
      {balances.length > 0 && (
        <div className="dock-balances">
          <span className="dock-balances__label">余额</span>
          {balances.map((row) => (
            <button
              key={`${row.provider}:${row.name}`}
              type="button"
              className="dock-balance"
              title={quotaTitle(row)}
              aria-label={`${shortName(row.name)} 余额 ${row.balance} · 点开看明细`}
              onClick={onOpen}
            >
              <span>{shortName(row.name)}</span>
              <b>{row.balance}</b>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export const DockChrome = memo(function DockChrome({
  page,
  onPage,
  onNewChat,
  memoCount,
  quotaRows,
  active,
  totalSessions,
  currentSessionId,
  doneUnreadIds,
  onOpenSession,
  onSuspendSession,
  onOpenPalette,
  onUndock,
  onQuit,
  updateAvailable,
  updateLabel,
  updateBusy,
  onCheckUpdate,
  onUpdateAndRelaunch,
  splitPaused,
}: Props) {
  const bandRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const now = Date.now();
  const anyRunning = active.some(isBusy);

  /**
   * The strip's title bar is the QQ-style "drag it off the edge" handle.
   *
   * Tauri's injected drag script only starts a drag when the mousedown target
   * itself carries `data-tauri-drag-region`, and it turns a double click into
   * `toggle_maximize` — which would throw a 400px docked strip into a maximized
   * window. Handle the press here instead: drag from anywhere that is not a
   * control, and never maximize. stopPropagation keeps the injected handler out
   * of the same press.
   */
  const onBarMouseDown = (event: React.MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (target?.closest("button, a, input, textarea, select, [role='menuitem'], [contenteditable='true']")) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    // The ⋯ menu closes on window mousedown; this press never reaches it.
    if (menuOpen) setMenuOpen(false);
    // detail 2 = second press of a double click: swallow it, do not maximize.
    if (event.detail > 1) return;
    beginDockBarDrag(); // arms undock-by-drag; own slides are ignored
    void getCurrentWindow().startDragging().catch(() => undefined);
  };

  // The app shell pads by the band's real height (quota rows and the active
  // list change it), measured instead of guessed.
  useLayoutEffect(() => {
    const el = bandRef.current;
    if (!el) return;
    const root = document.documentElement;
    const apply = () => root.style.setProperty("--dock-chrome-top", `${el.offsetHeight}px`);
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => {
      ro.disconnect();
      root.style.removeProperty("--dock-chrome-top");
    };
  }, []);

  useEffect(() => {
    if (!menuOpen) return;
    const onDown = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [menuOpen]);

  return (
    <div ref={bandRef} className="dock-chrome">
      <div
        className={page ? "dock-bar is-page" : "dock-bar"}
        onMouseDown={onBarMouseDown}
        title="拖动顶部可移出贴边"
      >
        {page ? (
          <>
            <button type="button" className="dock-bar__back" onClick={() => onPage(null)} title="返回 · Esc">
              <ChevronLeft size={14} />
              返回
            </button>
            <span className="dock-bar__title">
              {PAGE_TITLES[page]}
              {page === "memo" && memoCount > 0 && <small>{memoCount}</small>}
              {page === "shelf" && <small>{totalSessions}</small>}
            </span>
          </>
        ) : (
          <span className={anyRunning ? "dock-bar__brand is-live" : "dock-bar__brand"}>
            <i aria-hidden />
            Marionette
          </span>
        )}
        <span className="dock-bar__spacer" />
        {!page && (
          <button type="button" className="dock-icon" title="搜索对话与操作 · Ctrl K" aria-label="搜索对话与操作" onClick={onOpenPalette}>
            <Search size={13} />
          </button>
        )}
        <div className="dock-menu-anchor" ref={menuRef}>
          <button
            type="button"
            className={menuOpen ? "dock-icon is-on" : "dock-icon"}
            title="更多"
            aria-label="更多"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((open) => !open)}
          >
            <MoreHorizontal size={14} />
            {updateAvailable && <span className="dock-icon__pip" aria-hidden />}
          </button>
          {menuOpen && (
            <div className="dock-menu" role="menu">
              <button
                type="button"
                role="menuitem"
                className={updateAvailable ? "is-accent" : undefined}
                disabled={updateBusy}
                onClick={() => {
                  setMenuOpen(false);
                  if (updateAvailable) onUpdateAndRelaunch();
                  else onCheckUpdate();
                }}
              >
                {updateAvailable ? `升级到 ${updateLabel ?? "新版本"} 并重启` : updateBusy ? "检查中…" : "检查更新"}
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMenuOpen(false);
                  onPage("context");
                }}
              >
                用量与项目
                <small>额度 · 改动文件 · 待办</small>
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMenuOpen(false);
                  onUndock();
                }}
              >
                解除贴边
                <small>恢复普通窗口</small>
              </button>
              <span className="dock-menu__sep" />
              <button
                type="button"
                role="menuitem"
                className="is-danger"
                onClick={() => {
                  setMenuOpen(false);
                  onQuit();
                }}
              >
                退出 Marionette
                <small>停止引擎与全部会话</small>
              </button>
            </div>
          )}
        </div>
      </div>

      <div className="dock-actions">
        <button type="button" className="dock-btn dock-btn--primary" onClick={onNewChat}>
          <Plus size={13} />
          新建对话
        </button>
        <button type="button" className="dock-btn" onClick={() => onPage("memo")} title="收纳盒 · 拖入文件 / Ctrl V 粘贴文字与链接">
          <Archive size={13} />
          收纳盒
          {memoCount > 0 && <span className="dock-btn__count">{memoCount > 99 ? "99+" : memoCount}</span>}
        </button>
      </div>

      <QuotaMeters rows={quotaRows} onOpen={() => onPage("context")} />

      <div className="dock-active">
        <div className="dock-active__head">
          <span>进行中</span>
          <b>{active.filter(isAlive).length}</b>
          {splitPaused && <span className="dock-active__hint" title="分屏在贴边时暂停，解除贴边后恢复">分屏已暂停</span>}
          <button type="button" className="dock-active__all" onClick={() => onPage("shelf")}>
            全部对话 ›
          </button>
        </div>
        <div className="dock-active__list custom-scrollbar">
          {active.map((session) => {
            const busy = isBusy(session);
            const done = !busy && doneUnreadIds.has(session.id);
            const alive = isAlive(session);
            return (
              <div
                key={session.id}
                role="button"
                tabIndex={0}
                className={session.id === currentSessionId ? "dock-active__row is-current" : "dock-active__row"}
                title={session.label}
                onClick={() => onOpenSession(session)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") onOpenSession(session);
                }}
              >
                <span
                  className={
                    busy
                      ? `session-row__pulse is-${session.status}`
                      : done
                        ? "session-row__dot is-done"
                        : `session-row__dot is-${session.status}`
                  }
                  aria-hidden
                />
                <span className="dock-active__title">{session.label}</span>
                <span className="dock-active__time">{alive ? recency(session, now) : "未启动"}</span>
                {alive && (
                  <button
                    type="button"
                    className="dock-active__park"
                    title={busy ? "停止并挂起" : "挂起（结束进程，下次发消息自动接上）"}
                    aria-label={`挂起 ${session.label}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      onSuspendSession(session.id);
                    }}
                  >
                    <Pause size={11} />
                  </button>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
});
