/**
 * The docked strip's chrome (edge dock, docs/plan-edge-dock.md §3; layout per
 * docs/design/edge-dock.html ②).
 *
 * One fixed band above the conversation, top to bottom:
 *   title bar   brand · search · usage · ⋯   — or ‹ 返回 + page title on a page
 *   actions     新建对话 (destination picker: 聊天 / projects / 全部对话) · 收纳盒
 *   quota       one compact meter per subscription + balance chips, tap = usage page
 *   active      dialogs still holding a process + the one in view; replaces tabs.
 *   A finished dialog stays listed until it is really parked (2-hour
 *   auto-suspend, the row's ⏸, or agent exit) — App's dockActiveSessions.
 * Pages (all dialogs / usage / memo box) cover everything below the title bar
 * and come back with ‹ 返回 or Esc. ⋯ also holds 侧边栏常驻 (pinned strip),
 * undock / update / quit. The band's height is measured into --dock-chrome-top.
 * DockResizeGrip (below) is the inner-edge width handle App mounts on the
 * home view of an expanded strip.
 */

import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { ChevronDown, ChevronLeft, MoreHorizontal, Pause, Search } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { agentInkStyle } from "../lib/agentInk";
import type { EngineQuotaEntry, EngineQuotaWindow } from "../lib/api";
import { lastActiveMs } from "../lib/sessionSuspend";
import {
  applyDockWidth,
  beginDockBarDrag,
  getDockEdge,
  getDockWidth,
  persistDockWidth,
  readDockPrefs,
  subscribeDockPrefs,
  toggleDockPinned,
} from "../lib/dockMode";
import type { Session } from "../lib/types";
import { t, useT, type DictKey } from "../lib/i18n";

export type DockPage = "shelf" | "context" | "memo" | "doc" | "browser" | "usage";

const PAGE_TITLES: Record<DockPage, DictKey> = {
  shelf: "dock.pageShelf",
  context: "dock.pageContext",
  memo: "dock.pageMemo",
  doc: "dock.pageDoc",
  browser: "dock.pageBrowser",
  usage: "dock.pageUsage",
};

type Props = {
  page: DockPage | null;
  onPage: (page: DockPage | null) => void;
  onNewChat: () => void;
  /** 新建对话的目的地（2026-10-25）：在某个项目里开新会话。 */
  onNewChatIn: (projectId: string) => void;
  /** Projects offered as new-chat destinations (name + id only). */
  projects: Array<{ id: string; name: string }>;
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
  if (min < 1) return t("memo.justNow");
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
  if (hr < 1) return ` · ${t("dock.resetMinutes", { n: Math.max(1, Math.round(left / 60_000)) })}`;
  if (hr < 48) return ` · ${t("dock.resetHours", { n: hr })}`;
  return ` · ${t("dock.resetDays", { n: Math.round(hr / 24) })}`;
}

/** Hover text is the full truth: every window + reset time, then the balance. */
function quotaTitle(row: EngineQuotaEntry): string {
  return [
    row.name,
    ...row.windows.map((w) => {
      const used = usedOf(w);
      return `${w.name}  ${used == null ? "—" : `${Math.round(used)}%`}${resetLabel(w.resetsAt)}`;
    }),
    ...(row.balance ? [t("dock.balanceChip", { balance: row.balance })] : []),
  ].join("\n");
}

function windowsOf(row: EngineQuotaEntry): Array<{ w: EngineQuotaWindow; used: number }> {
  return row.windows
    .map((w) => ({ w, used: usedOf(w) }))
    .filter((x): x is { w: EngineQuotaWindow; used: number } => x.used != null)
    .sort((a, b) => b.used - a.used);
}

/**
 * The v2 quota pattern (docs/design/ui-v2/DockV2.html): the tightest source is
 * one block — name 14px/600, reset faint right, then a 4px ink bar row for its
 * tightest window and 2px grey rows for the rest — and every other source
 * collapses into one line of 12px text items. Details stay on hover
 * (quotaTitle) / the usage page. quotaRows carries no agent→provider mapping,
 * so the block carries the current session agent's ink.
 */
function QuotaMeter({ row, agentId, onOpen }: { row: EngineQuotaEntry; agentId: string | null; onOpen: () => void }) {
  const t = useT();
  const windows = windowsOf(row);
  const peak = windows[0]?.used ?? null;
  const name = shortName(row.name);
  const reset = resetLabel(windows[0]?.w.resetsAt);
  return (
    <button
      type="button"
      className={`dock-quota__main ${tone(peak)}`}
      style={agentInkStyle(agentId)}
      title={quotaTitle(row)}
      aria-label={`${name} ${peak == null ? t("dock.noQuotaData") : `${Math.round(peak)}%`} · ${t("dock.openDetails")}`}
      onClick={onOpen}
    >
      <span className="dock-quota__head">
        <span className="dock-quota__name">{name}</span>
        {reset && <span className="dock-quota__reset">{reset.slice(3)}</span>}
      </span>
      {windows.map((x, i) => (
        <span key={x.w.name} className={i === 0 ? "dock-quota__wr is-main" : "dock-quota__wr"}>
          <span className="dock-quota__wr-name">{x.w.name}</span>
          <i aria-hidden>
            <b style={{ width: `${x.used}%` }} />
          </i>
          <em>{Math.round(x.used)}%</em>
        </span>
      ))}
    </button>
  );
}

/** One line of 12px items: `Codex 26%  GLM 87%(yellow)  opencode 已满(red)`. */
function QuotaLineItems({ rows, onOpen }: { rows: EngineQuotaEntry[]; onOpen: () => void }) {
  const t = useT();
  return (
    <div className="dock-quota__line">
      {rows.map((row) => {
        const peak = windowsOf(row)[0]?.used ?? null;
        const full = peak != null && peak >= 100;
        return (
          <button
            key={`${row.provider}:${row.name}`}
            type="button"
            className={full ? "is-full" : peak != null && peak >= 75 ? "is-warm" : undefined}
            title={quotaTitle(row)}
            aria-label={`${shortName(row.name)} ${peak == null ? t("dock.noQuotaData") : `${Math.round(peak)}%`} · ${t("dock.openDetails")}`}
            onClick={onOpen}
          >
            {shortName(row.name)} {peak == null ? "—" : full ? t("v2dock.quotaFull") : `${Math.round(peak)}%`}
          </button>
        );
      })}
    </div>
  );
}

function QuotaMeters({ rows, agentId, onOpen }: { rows: EngineQuotaEntry[]; agentId: string | null; onOpen: () => void }) {
  const t = useT();
  const sorted = useMemo(() => {
    const peak = (row: EngineQuotaEntry) => windowsOf(row)[0]?.used ?? -1;
    return [...rows].sort((a, b) => peak(b) - peak(a));
  }, [rows]);
  if (sorted.length === 0) return null;
  // The tightest source with windows becomes the block; balance-only sources
  // (the amount is the whole story, not an empty 0% bar) join the line too.
  const balances = sorted.filter((row) => row.balance && windowsOf(row).length === 0);
  const meters = sorted.filter((row) => !balances.includes(row));
  const [primary, ...rest] = meters;
  return (
    <div className="dock-quota">
      {primary && <QuotaMeter row={primary} agentId={agentId} onOpen={onOpen} />}
      {rest.length > 0 && <QuotaLineItems rows={rest} onOpen={onOpen} />}
      {balances.length > 0 && (
        <div className="dock-quota__line">
          <span className="dock-quota__line-label">{t("dock.balanceLabel")}</span>
          {balances.map((row) => (
            <button
              key={`${row.provider}:${row.name}`}
              type="button"
              className="dock-quota__bal"
              title={quotaTitle(row)}
              aria-label={`${shortName(row.name)} ${t("dock.balanceLabel")} ${row.balance} · ${t("dock.openDetails")}`}
              onClick={onOpen}
            >
              {shortName(row.name)}
              <b>{row.balance}</b>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * Strip menus portal to <body>. `.dock-chrome` carries its own z-index (40),
 * which makes it a stacking context — a menu inside the band can never paint
 * above the page overlays (70/75), so 全部对话 opened *over* the ⋯ rows and the
 * menu looked like it never opened. Fixed to the anchor's rect keeps the same
 * look while escaping that context; the panel ref feeds the outside-click guard
 * (a portaled node is no longer inside the anchor).
 */
function DockMenuLayer({
  anchorRef,
  align = "right",
  panelRef,
  className,
  children,
}: {
  anchorRef: RefObject<HTMLDivElement | null>;
  align?: "left" | "right";
  /** Writable: the panel node is handed back for the outside-click guard. */
  panelRef: { current: HTMLDivElement | null };
  className?: string;
  children: ReactNode;
}) {
  const localRef = useRef<HTMLDivElement | null>(null);
  const [box, setBox] = useState<{ top: number; left: number; maxHeight: number } | null>(null);

  useLayoutEffect(() => {
    const place = () => {
      const anchor = anchorRef.current;
      const panel = localRef.current;
      if (!anchor) return;
      const rect = anchor.getBoundingClientRect();
      const width = panel?.offsetWidth || 196;
      const rawLeft = align === "left" ? rect.left : rect.right - width;
      const top = rect.bottom + 4;
      setBox({
        top,
        left: Math.min(Math.max(8, rawLeft), Math.max(8, window.innerWidth - width - 8)),
        maxHeight: Math.max(160, window.innerHeight - top - 12),
      });
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [anchorRef, align]);

  return createPortal(
    <div
      ref={(node) => {
        localRef.current = node;
        panelRef.current = node;
      }}
      className={className ? `dock-menu ${className}` : "dock-menu"}
      role="menu"
      style={box ? { position: "fixed", top: box.top, left: box.left, right: "auto", maxHeight: box.maxHeight } : { visibility: "hidden" }}
    >
      {children}
    </div>,
    document.body,
  );
}

export const DockChrome = memo(function DockChrome({
  page,
  onPage,
  onNewChat,
  onNewChatIn,
  projects,
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
  const t = useT();
  const bandRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const newChatRef = useRef<HTMLDivElement>(null);
  /** Portaled panels — outside the anchors, so the close guard needs them too. */
  const morePanelRef = useRef<HTMLDivElement | null>(null);
  const newPanelRef = useRef<HTMLDivElement | null>(null);
  /** One popover at a time: the ⋯ menu or the new-chat destination picker. */
  const [openMenu, setOpenMenu] = useState<"more" | "new" | null>(null);
  /** 侧边栏常驻 — mirrored from prefs so the ⋯ row can draw its state. */
  const [pinned, setPinned] = useState(() => readDockPrefs().pinned);
  useEffect(() => subscribeDockPrefs((prefs) => setPinned(prefs.pinned)), []);
  const now = Date.now();
  const anyRunning = active.some(isBusy);
  /** quotaRows has no agent→provider mapping; the current agent's ink stands in. */
  const currentAgent = active.find((session) => session.id === currentSessionId)?.agentId ?? null;

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
    // Menus close on window mousedown; this press never reaches it.
    if (openMenu) setOpenMenu(null);
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
    if (!openMenu) return;
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (
        !menuRef.current?.contains(target) &&
        !newChatRef.current?.contains(target) &&
        !morePanelRef.current?.contains(target) &&
        !newPanelRef.current?.contains(target)
      ) {
        setOpenMenu(null);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpenMenu(null);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [openMenu]);

  return (
    <div ref={bandRef} className="dock-chrome">
      <div
        className={page ? "dock-bar is-page" : "dock-bar"}
        onMouseDown={onBarMouseDown}
        title={t("dock.dragToUndock")}
      >
        {page ? (
          <>
            <button type="button" className="dock-bar__back" onClick={() => onPage(null)} title={t("dock.backTitle")}>
              <ChevronLeft size={14} />
              {t("dock.back")}
            </button>
            <span className="dock-bar__title">
              {t(PAGE_TITLES[page])}
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
          <button type="button" className="dock-icon" title={t("dock.searchTitle")} aria-label={t("dock.searchAria")} onClick={onOpenPalette}>
            <Search size={14} />
          </button>
        )}
        <div className="dock-menu-anchor" ref={menuRef}>
          <button
            type="button"
            className={openMenu === "more" ? "dock-icon is-on" : "dock-icon"}
            title={t("dock.more")}
            aria-label={t("dock.more")}
            aria-haspopup="menu"
            aria-expanded={openMenu === "more"}
            onClick={() => setOpenMenu(openMenu === "more" ? null : "more")}
          >
            <MoreHorizontal size={14} />
            {updateAvailable && <span className="dock-icon__pip" aria-hidden />}
          </button>
          {openMenu === "more" && (
            <DockMenuLayer anchorRef={menuRef} panelRef={morePanelRef}>
              <button
                type="button"
                role="menuitem"
                className={updateAvailable ? "is-accent" : undefined}
                disabled={updateBusy}
                onClick={() => {
                  setOpenMenu(null);
                  if (updateAvailable) onUpdateAndRelaunch();
                  else onCheckUpdate();
                }}
              >
                {updateAvailable
                  ? t("dock.updateRelaunch", { label: updateLabel ?? t("dock.newVersion") })
                  : updateBusy
                    ? t("dock.checking")
                    : t("dock.checkUpdate")}
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setOpenMenu(null);
                  onPage("context");
                }}
              >
                {t("dock.pageContext")}
                <small>{t("dock.contextHint")}</small>
              </button>
              {/* 侧边栏常驻 stays open — the row itself is the feedback. */}
              <button
                type="button"
                role="menuitem"
                aria-pressed={pinned}
                className={pinned ? "is-accent" : undefined}
                onClick={() => setPinned(toggleDockPinned())}
              >
                {t("dock.pinnedStrip")}
                {pinned ? ` · ${t("dock.pinnedOn")}` : ""}
                <small>{pinned ? t("dock.pinnedHintOn") : t("dock.pinnedHintOff")}</small>
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setOpenMenu(null);
                  onUndock();
                }}
              >
                {t("dock.undock")}
                <small>{t("dock.undockHint")}</small>
              </button>
              <span className="dock-menu__sep" />
              <button
                type="button"
                role="menuitem"
                className="is-danger"
                onClick={() => {
                  setOpenMenu(null);
                  onQuit();
                }}
              >
                {t("dock.quit")}
                <small>{t("dock.quitHint")}</small>
              </button>
            </DockMenuLayer>
          )}
        </div>
      </div>

      <div className="dock-actions">
        {/* 新建对话 opens a destination picker (2026-10-25): plain click used to
            always create in 聊天 — now 聊天 is just the first destination and
            each project is one tap away. 全部对话 keeps the full-list path. */}
        <div className="dock-menu-anchor dock-new-anchor" ref={newChatRef}>
          <button
            type="button"
            className="dock-btn dock-btn--primary"
            aria-haspopup="menu"
            aria-expanded={openMenu === "new"}
            onClick={() => setOpenMenu(openMenu === "new" ? null : "new")}
          >
            {t("dock.newChat")}
            <ChevronDown size={10} />
          </button>
          {openMenu === "new" && (
            <DockMenuLayer anchorRef={newChatRef} align="left" panelRef={newPanelRef}>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setOpenMenu(null);
                  onNewChat();
                }}
              >
                {t("common.chat")}
                <small>{t("dock.destChatHint")}</small>
              </button>
              {projects.map((project) => (
                <button
                  key={project.id}
                  type="button"
                  role="menuitem"
                  title={t("dock.newInProject", { name: project.name })}
                  onClick={() => {
                    setOpenMenu(null);
                    onNewChatIn(project.id);
                  }}
                >
                  {project.name}
                </button>
              ))}
              {projects.length === 0 && (
                <span className="dock-menu__hint">{t("dock.noProjects")}</span>
              )}
              <span className="dock-menu__sep" />
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setOpenMenu(null);
                  onPage("shelf");
                }}
              >
                {t("dock.pageShelf")} ›
                <small>{t("dock.allChatsHint")}</small>
              </button>
            </DockMenuLayer>
          )}
        </div>
        <button type="button" className="dock-btn" onClick={() => onPage("memo")} title={t("dock.memoButtonTitle")}>
          {t("dock.pageMemo")}
          {memoCount > 0 && <span className="dock-btn__count">{memoCount > 99 ? "99+" : memoCount}</span>}
        </button>
      </div>

      <QuotaMeters rows={quotaRows} agentId={currentAgent} onOpen={() => onPage("context")} />

      <div className="dock-active">
        <div className="dock-active__head">
          <span>{t("dock.active")}</span>
          <b>{active.filter(isAlive).length}</b>
          {splitPaused && (
            <span className="dock-active__hint" title={t("dock.splitPausedTitle")}>
              {t("dock.splitPaused")}
            </span>
          )}
          <button type="button" className="dock-active__all" onClick={() => onPage("shelf")}>
            {t("dock.pageShelf")} ›
          </button>
        </div>
        <div className="dock-active__list custom-scrollbar">
          {active.map((session) => {
            const busy = isBusy(session);
            const done = !busy && doneUnreadIds.has(session.id);
            const alive = isAlive(session);
            const waiting = session.status === "waiting";
            const errored = session.status === "error";
            const since = recency(session, now);
            return (
              <div
                key={session.id}
                role="button"
                tabIndex={0}
                className={session.id === currentSessionId ? "dock-active__row is-current" : "dock-active__row"}
                style={agentInkStyle(session.agentId)}
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
                <span
                  className={`dock-active__time${
                    busy ? " is-running" : waiting ? " is-waiting" : errored ? " is-errored" : done ? " is-done" : ""
                  }`}
                >
                  {busy
                    ? since
                      ? `${t("v2dock.running")} · ${since}`
                      : t("v2dock.running")
                    : waiting
                      ? t("v2dock.waiting")
                      : errored
                        ? t("v2dock.errored")
                        : done
                          ? since
                            ? `${t("v2dock.done")} · ${since}`
                            : t("v2dock.done")
                          : alive
                            ? since
                            : t("dock.notStarted")}
                </span>
                {alive && (
                  <button
                    type="button"
                    className="dock-active__park"
                    title={busy ? t("dock.stopAndPark") : t("dock.parkTitle")}
                    aria-label={t("dock.parkAria", { label: session.label })}
                    onClick={(event) => {
                      event.stopPropagation();
                      onSuspendSession(session.id);
                    }}
                  >
                    <Pause size={12} />
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

/**
 * The strip's inner-edge width grip (2026-10-25): fixed to the viewport side
 * away from the screen edge, full height, rendered by App only while
 * docked-expanded with no page open. Pointer-captured drag calls
 * applyDockWidth per move (the window itself resizes — clientX is measured
 * against the live client rect, see dockMode.ts); release persists the width.
 */
export const DockResizeGrip = memo(function DockResizeGrip() {
  const t = useT();
  const [dragging, setDragging] = useState(false);
  const draggingRef = useRef(false);

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      // pointer already gone — the press simply does nothing
    }
    draggingRef.current = true;
    setDragging(true);
    document.documentElement.classList.add("dock-resizing");
  };

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    const current = getDockWidth();
    // Right dock drags its left (inner) edge: wider = clientX went negative.
    // Left dock drags its right edge: the width is the pointer's clientX.
    const next = getDockEdge() === "right" ? current - event.clientX : event.clientX;
    applyDockWidth(next);
  };

  const endDrag = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    setDragging(false);
    document.documentElement.classList.remove("dock-resizing");
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      // already released by the browser
    }
    persistDockWidth();
  };

  return (
    <div
      className={dragging ? "dock-resize-grip is-dragging" : "dock-resize-grip"}
      role="separator"
      aria-orientation="vertical"
      aria-label={t("dock.resizeAria")}
      title={t("dock.resizeAria")}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
    />
  );
});
