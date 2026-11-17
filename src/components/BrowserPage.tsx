import { useCallback, useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { Bot, Crosshair, Globe, Plus, RefreshCw, Share2, X } from "lucide-react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  browserCloseTab,
  browserNavigateTab,
  browserOpenTab,
  browserPickClear,
  browserPickFinalize,
  browserPickResolve,
  browserPickSet,
  browserSetTabBounds,
  browserSetTabVisible,
  browserShareTab,
  onBrowserPick,
  type BrowserPickEvent,
  type BrowserPickMode,
  type BrowserPermissionRequest,
  type BrowserTab,
} from "../lib/browserTabs";
import {
  composePickPrompt,
  elementShort,
  pickTargetLabel,
  BROWSER_PICK_LIMIT,
  type BrowserPickModes,
  type BrowserPickRow,
  type BrowserPickRowsByTab,
  type BrowserPickRefs,
} from "../lib/browserPick";
import { useT } from "../lib/i18n";

/**
 * The built-in browser page: a strip of tabs over a native surface host.
 * The page itself is DOM; each tab's web content is a wry child webview the
 * Rust side positions to match the host's rectangle (ResizeObserver →
 * browser_set_tab_bounds — logical CSS px, the same space WebView2 bounds
 * use). Only the active tab is visible, and none are while the page is
 * covered by a conversation.
 */
type BrowserPageProps = {
  tabs: BrowserTab[];
  activeTabId: string | null;
  onSelectTab: (tabId: string) => void;
  /** The page is the visible center page — surfaces show only when true. */
  visible: boolean;
  /** Esc only belongs to browser chrome when embedded beside a chat pane. */
  embedded?: boolean;
  /** Esc: leave the page (tabs stay open; the dock mount maps it to 返回). */
  onExit?: () => void;
  /** 批注抽屉的「发给对话」：成功后才允许 drawer 丢弃草稿。 */
  onSendToChat?: (prompt: string, imagePaths: string[]) => Promise<boolean>;
  pickModes: BrowserPickModes;
  picksByTab: BrowserPickRowsByTab;
  setPickModes: Dispatch<SetStateAction<BrowserPickModes>>;
  setPicksByTab: Dispatch<SetStateAction<BrowserPickRowsByTab>>;
};

const FINALIZE_RETRIES = 6;
const FINALIZE_RETRY_MS = 400;
const PICKER_TOGGLE_RETRIES = 6;
const PICKER_TOGGLE_RETRY_MS = 350;

async function setPickerBackend(tabId: string, mode: BrowserPickMode | null) {
  let result = await browserPickSet(tabId, mode !== null, mode ?? "element");
  for (let attempt = 1; attempt < PICKER_TOGGLE_RETRIES && !result.ok; attempt++) {
    await new Promise((resolve) => setTimeout(resolve, PICKER_TOGGLE_RETRY_MS));
    result = await browserPickSet(tabId, mode !== null, mode ?? "element");
  }
  return result;
}

async function clearPickerBackend(tabId: string): Promise<boolean> {
  for (let attempt = 0; attempt < PICKER_TOGGLE_RETRIES; attempt++) {
    if (await browserPickClear(tabId)) return true;
    await new Promise((resolve) => setTimeout(resolve, PICKER_TOGGLE_RETRY_MS));
  }
  return false;
}

export function BrowserPage({
  tabs,
  activeTabId,
  onSelectTab,
  visible,
  embedded = false,
  onExit,
  onSendToChat,
  pickModes,
  picksByTab,
  setPickModes,
  setPicksByTab,
}: BrowserPageProps) {
  const t = useT();
  const rootRef = useRef<HTMLDivElement>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  const [address, setAddress] = useState("");
  const [shareMenuFor, setShareMenuFor] = useState<string | null>(null);
  const active = tabs.find((tab) => tab.id === activeTabId) ?? tabs[0] ?? null;
  const [resolving, setResolving] = useState(false);
  const [pickerNotice, setPickerNotice] = useState<string | null>(null);
  const pickerMode = active ? (pickModes[active.id] ?? null) : null;
  const pickRows = active ? (picksByTab[active.id] ?? []) : [];
  const pendingScreenshots = pickRows.some((row) => row.screenshotPending);
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const picksRef = useRef(picksByTab);
  picksRef.current = picksByTab;
  const pickModesRef = useRef(pickModes);
  pickModesRef.current = pickModes;
  const pickerRequestSeq = useRef<Record<string, number>>({});

  // Address bar follows the active tab; typing is a draft, Enter navigates.
  useEffect(() => {
    setAddress(active?.url ?? "");
  }, [active?.id, active?.url]);

  useEffect(() => {
    setPickerNotice(null);
  }, [active?.id]);

  const syncBounds = async (tabId: string): Promise<boolean> => {
    const host = hostRef.current;
    if (!host) return false;
    const rect = host.getBoundingClientRect();
    if (rect.width < 40 || rect.height < 40) return false;
    try {
      await browserSetTabBounds(tabId, rect.x, rect.y, rect.width, rect.height);
      return true;
    } catch {
      return false;
    }
  };

  // Surface visibility: the active tab shows when the page shows; everything
  // else hides. A tab is only made visible AFTER its bounds landed — the
  // native surface starts as a 1×1 dot and a failed measurement must keep it
  // that way, never show it over the app chrome (the 2026-10-31 freeze).
  const shownRef = useRef<string | null>(null);
  useEffect(() => {
    const previous = shownRef.current;
    const target = visible ? (active?.id ?? null) : null;
    if (previous && previous !== target) {
      void browserSetTabVisible(previous, false);
    }
    shownRef.current = target;
    if (!target) return;
    let cancelled = false;
    // Layout can lag a render: retry the measurement for up to ~2s until the
    // host has a real rectangle, then place and reveal.
    (async () => {
      for (let attempt = 0; attempt < 20 && !cancelled; attempt++) {
        if (await syncBounds(target)) {
          if (!cancelled) void browserSetTabVisible(target, true);
          return;
        }
        await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [active?.id, visible]);

  useEffect(
    () => () => {
      if (shownRef.current) void browserSetTabVisible(shownRef.current, false);
    },
    [],
  );

  // The native page survives while this React page unmounts. Stop every
  // active in-page picker before dropping the UI that could turn it off; the
  // annotation rows themselves live in App and survive the remount.
  useEffect(
    () => () => {
      const tabIds = new Set([
        ...Object.keys(pickModesRef.current),
        ...Object.keys(pickerRequestSeq.current),
      ]);
      for (const tabId of tabIds) {
        pickerRequestSeq.current[tabId] = (pickerRequestSeq.current[tabId] ?? 0) + 1;
        void setPickerBackend(tabId, null);
      }
      setPickModes((prev) => {
        let changed = false;
        const next: BrowserPickModes = { ...prev };
        for (const tabId of Object.keys(next)) {
          if (next[tabId]) {
            next[tabId] = null;
            changed = true;
          }
        }
        return changed ? next : prev;
      });
    },
    [setPickModes],
  );

  // Rectangle sync: CSS px of the host == logical px of the window client
  // area (the app webview fills the window), which is the space wry bounds
  // expect. The drawer below shrinks the host; this keeps the webview honest.
  useEffect(() => {
    const host = hostRef.current;
    if (!host || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (active && visible) void syncBounds(active.id);
    });
    observer.observe(host);
    return () => observer.disconnect();
  }, [active?.id, visible]);

  const openNewTab = () => {
    const url = address.trim() || "https://www.bing.com";
    void browserOpenTab(url).then((result) => {
      if (result) onSelectTab(result.tabId);
    });
  };

  const closeTab = (tabId: string) => {
    const remaining = tabs.filter((tab) => tab.id !== tabId);
    if (remaining[0]) onSelectTab(remaining[0].id);
    setPickModes((prev) => {
      if (!(tabId in prev)) return prev;
      const next = { ...prev };
      delete next[tabId];
      return next;
    });
    setPicksByTab((prev) => {
      if (!(tabId in prev)) return prev;
      const next = { ...prev };
      delete next[tabId];
      return next;
    });
    void browserCloseTab(tabId);
  };

  const submitAddress = () => {
    if (!active) {
      openNewTab();
      return;
    }
    const url = address.trim();
    if (url) void browserNavigateTab(active.id, url);
  };

  const activeGrant = active?.grant ?? "none";

  // ── element picker (annotate mode) ───────────────────────────────────────

  const setPickerMode = useCallback(
    async (tabId: string, mode: BrowserPickMode | null): Promise<boolean> => {
      const request = (pickerRequestSeq.current[tabId] ?? 0) + 1;
      pickerRequestSeq.current[tabId] = request;
      const previousMode = pickModesRef.current[tabId] ?? null;
      setPickerNotice(null);
      if (mode === null) {
        setPickModes((prev) => (prev[tabId] ? { ...prev, [tabId]: null } : prev));
      }
      const result = await setPickerBackend(tabId, mode);
      if (pickerRequestSeq.current[tabId] !== request) return false;
      if (!tabsRef.current.some((tab) => tab.id === tabId)) return false;
      if (!result.ok) {
        if (mode === null && previousMode) {
          setPickModes((prev) => ({ ...prev, [tabId]: previousMode }));
        }
        setPickerNotice(t(mode === null ? "picker.stopFailed" : "picker.startFailed"));
        return false;
      }
      setPickModes((prev) => (prev[tabId] === mode ? prev : { ...prev, [tabId]: mode }));
      return true;
    },
    [setPickModes, t],
  );

  const togglePicker = () => {
    if (!active) return;
    const current = pickModes[active.id] ?? null;
    void setPickerMode(active.id, current ? null : "element");
  };

  // A crop can land while another tab's CDP call holds the lease — retry a
  // few times before showing the failed chip.
  const finalizeScreenshot = useCallback(async (tabId: string, pickId: string): Promise<boolean> => {
    for (let attempt = 0; attempt < FINALIZE_RETRIES; attempt++) {
      const result = await browserPickFinalize(tabId, pickId);
      if (result) {
        setPicksByTab((prev) => {
          if (!prev[tabId]) return prev;
          return {
            ...prev,
            [tabId]: prev[tabId].map((row) =>
              row.pickId === pickId
                ? {
                    ...row,
                    screenshot: result.dataUrl,
                    screenshotPath: result.path,
                    screenshotPending: false,
                    screenshotFailed: false,
                  }
                : row,
            ),
          };
        });
        return true;
      }
      await new Promise((resolve) => setTimeout(resolve, FINALIZE_RETRY_MS));
    }
    setPicksByTab((prev) => {
      if (!prev[tabId]) return prev;
      return {
        ...prev,
        [tabId]: prev[tabId].map((row) =>
          row.pickId === pickId
            ? { ...row, screenshotPending: false, screenshotFailed: true }
            : row,
        ),
      };
    });
    return false;
  }, [setPicksByTab]);

  const retryScreenshot = (tabId: string, pickId: string) => {
    setPicksByTab((prev) => {
      if (!prev[tabId]) return prev;
      return {
        ...prev,
        [tabId]: prev[tabId].map((row) =>
          row.pickId === pickId
            ? { ...row, screenshotPending: true, screenshotFailed: false }
            : row,
        ),
      };
    });
    void finalizeScreenshot(tabId, pickId);
  };

  // Picks travel page → Rust event → drawer; the crop (and everything else
  // CDP) is frontend-driven from here.
  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;
    void onBrowserPick((event: BrowserPickEvent) => {
      const { tabId, message } = event;
      if (message.kind === "cancel") {
        pickerRequestSeq.current[tabId] = (pickerRequestSeq.current[tabId] ?? 0) + 1;
        setPickModes((prev) => (prev[tabId] ? { ...prev, [tabId]: null } : prev));
        return;
      }
      if (message.kind === "limit") {
        pickerRequestSeq.current[tabId] = (pickerRequestSeq.current[tabId] ?? 0) + 1;
        setPickModes((prev) => (prev[tabId] ? { ...prev, [tabId]: null } : prev));
        setPickerNotice(t("picker.limit", { limit: message.limit }));
        return;
      }
      if ((picksRef.current[tabId] ?? []).length >= BROWSER_PICK_LIMIT) {
        setPickerNotice(t("picker.limit", { limit: BROWSER_PICK_LIMIT }));
        void setPickerMode(tabId, null);
        return;
      }
      const tab = tabsRef.current.find((entry) => entry.id === tabId);
      if (!tab) return;
      const url = message.url || tab.url;
      const row: BrowserPickRow =
        message.kind === "pick"
          ? {
              pickId: message.pickId,
              kind: "element",
              url,
              info: message.info,
              note: "",
              screenshot: null,
              screenshotPending: true,
            }
          : {
              pickId: message.pickId,
              kind: "region",
              url,
              rect: message.rect,
              elements: message.elements,
              scanCapped: message.scanCapped === true,
              note: "",
              screenshot: null,
              screenshotPending: true,
            };
      setPicksByTab((prev) => {
        const list = prev[tabId] ?? [];
        if (list.some((existing) => existing.pickId === message.pickId)) return prev;
        return { ...prev, [tabId]: [...list, row] };
      });
      void finalizeScreenshot(tabId, message.pickId);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [finalizeScreenshot, setPickModes, setPickerMode, setPicksByTab, t]);

  // A navigation stops the page picker on the backend; mirror that here so
  // the crosshair stops glowing on a page that no longer picks.
  const urlsRef = useRef<Record<string, string>>({});
  useEffect(() => {
    for (const tab of tabs) {
      if (urlsRef.current[tab.id] !== undefined && urlsRef.current[tab.id] !== tab.url) {
        pickerRequestSeq.current[tab.id] = (pickerRequestSeq.current[tab.id] ?? 0) + 1;
        if (active?.id === tab.id) setPickerNotice(null);
        setPickModes((prev) => (prev[tab.id] ? { ...prev, [tab.id]: null } : prev));
        void setPickerBackend(tab.id, null).then((result) => {
          if (!result.ok) setPickerNotice(t("picker.stopFailed"));
        });
      }
      urlsRef.current[tab.id] = tab.url;
    }
  }, [active?.id, setPickModes, t, tabs]);

  const sendToChat = async () => {
    const tab = active;
    if (!tab || !onSendToChat || resolving) return;
    const rows = picksByTab[tab.id] ?? [];
    if (rows.length === 0) return;
    if (rows.some((row) => row.screenshotPending)) {
      setPickerNotice(t("picker.waitScreenshots"));
      return;
    }
    setResolving(true);
    setPickerNotice(null);
    try {
      const resolved = await browserPickResolve(tab.id);
      const refs: BrowserPickRefs = resolved?.refs ?? {};
      const generation = resolved?.generation ?? "";
      const withShot = rows.filter((row) => row.screenshotPath);
      const prompt = composePickPrompt(rows, refs, generation, withShot.map((row) => row.pickId));
      const sent = await onSendToChat(
        prompt,
        withShot.map((row) => row.screenshotPath as string),
      );
      if (!sent) {
        setPickerNotice(t("picker.sendFailed"));
        return;
      }
      setPicksByTab((prev) => ({ ...prev, [tab.id]: [] }));
      setPickModes((prev) => ({ ...prev, [tab.id]: null }));
      if (!(await clearPickerBackend(tab.id))) {
        setPickerNotice(t("picker.stopFailed"));
      }
    } finally {
      setResolving(false);
    }
  };

  const removePick = (pickId: string) => {
    if (!active) return;
    setPicksByTab((prev) => ({
      ...prev,
      [active.id]: (prev[active.id] ?? []).filter((row) => row.pickId !== pickId),
    }));
  };

  const clearBatch = async () => {
    if (!active) return;
    const tabId = active.id;
    setPicksByTab((prev) => ({ ...prev, [tabId]: [] }));
    setPickerNotice(null);
    await setPickerMode(tabId, null);
    if (!(await clearPickerBackend(tabId))) {
      setPickerNotice(t("picker.stopFailed"));
    }
  };

  // Esc leaves the page (a share menu consumes it first, then the picker).
  // The page picker has its own Esc path — the in-page script sends a cancel
  // event that resets `pickModes` — so this only fires when the main window
  // holds focus (address bar, drawer inputs).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (
          embedded &&
          event.target instanceof Node &&
          rootRef.current &&
          !rootRef.current.contains(event.target)
        ) {
          return;
        }
        if (shareMenuFor) {
          setShareMenuFor(null);
          return;
        }
        if (active && (pickModes[active.id] ?? null)) {
          void setPickerMode(active.id, null);
          return;
        }
        onExit?.();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, embedded, pickModes, setPickerMode, onExit, shareMenuFor]);

  const drawerOpen =
    Boolean(active) && (pickerMode !== null || pickRows.length > 0 || pickerNotice !== null);

  return (
    <div ref={rootRef} className="browser-page">
      <div className="browser-page__bar">
        <div className="browser-page__tabs" role="tablist">
          {tabs.map((tab) => (
            <div
              key={tab.id}
              className={`browser-tab${tab.id === active?.id ? " is-active" : ""}`}
              role="tab"
              aria-selected={tab.id === active?.id}
            >
              <button
                type="button"
                className="browser-tab__select"
                title={tab.url}
                onClick={() => onSelectTab(tab.id)}
              >
                <Globe size={11} />
                <span>{tab.title || tab.origin || tab.url}</span>
                {tab.openedByAgent && (
                  <i className="browser-tab__agent" title={t("browser.openedByAgent")}>
                    <Bot size={10} />
                  </i>
                )}
                {tab.grant !== "none" && (
                  <i
                    className={`browser-tab__grant is-${tab.grant}`}
                    title={t(`browser.shareLabel.${tab.grant}` as never)}
                  />
                )}
              </button>
              <button
                type="button"
                className="pill-action pill-action--icon pill-action--sm browser-tab__close"
                title={t("browser.closeTab")}
                onClick={() => closeTab(tab.id)}
              >
                <X size={11} />
              </button>
            </div>
          ))}
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm browser-page__new"
            title={t("browser.newTab")}
            onClick={openNewTab}
          >
            <Plus size={12} />
          </button>
        </div>
        {active && (
          <div className="browser-page__nav">
            <input
              className="browser-page__address"
              value={address}
              placeholder={t("browser.addressHint")}
              spellCheck={false}
              onChange={(event) => setAddress(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  submitAddress();
                }
              }}
            />
            <button
              type="button"
              className={`pill-action pill-action--icon pill-action--sm browser-page__pick${
                pickerMode ? " is-active" : ""
              }`}
              title={t("picker.title")}
              aria-label={t("picker.title")}
              aria-pressed={Boolean(pickerMode)}
              onClick={togglePicker}
            >
              <Crosshair size={12} />
            </button>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              title={t("browser.reload")}
              onClick={() => void browserNavigateTab(active.id, active.url)}
            >
              <RefreshCw size={12} />
            </button>
            <div className="browser-page__share">
              <button
                type="button"
                className={`pill-action pill-action--sm browser-page__share-btn is-${activeGrant}`}
                onClick={() => setShareMenuFor((value) => (value ? null : active.id))}
              >
                <Share2 size={12} />
                <span>{t(`browser.shareLabel.${activeGrant}` as never)}</span>
              </button>
              {shareMenuFor === active.id && (
                <>
                  <div className="popover-scrim" onClick={() => setShareMenuFor(null)} />
                  <div className="browser-share-menu" role="menu">
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setShareMenuFor(null);
                        void browserShareTab(active.id, "none");
                      }}
                    >
                      {t("browser.shareNone")}
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setShareMenuFor(null);
                        void browserShareTab(active.id, "read");
                      }}
                    >
                      {t("browser.shareRead")}
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setShareMenuFor(null);
                        void browserShareTab(active.id, "control");
                      }}
                    >
                      {t("browser.shareControl")}
                    </button>
                  </div>
                </>
              )}
            </div>
          </div>
        )}
      </div>
      {active && drawerOpen && (
        <div className="browser-page__picker">
          <div className="browser-page__picker-head">
            {pickerMode ? (
              <>
                <div className="browser-page__picker-modes" role="tablist">
                  {(["element", "region"] as const).map((mode) => (
                    <button
                      key={mode}
                      type="button"
                      role="tab"
                      aria-selected={pickerMode === mode}
                      className={`browser-page__picker-mode${pickerMode === mode ? " is-active" : ""}`}
                      onClick={() => void setPickerMode(active.id, mode)}
                    >
                      {t(mode === "element" ? "picker.modeElement" : "picker.modeRegion")}
                    </button>
                  ))}
                </div>
                <span
                  className={`browser-page__picker-hint${pickerNotice ? " is-error" : ""}`}
                  role="status"
                  aria-live="polite"
                >
                  {pickerNotice ||
                    t(pickerMode === "element" ? "picker.hint" : "picker.hintRegion")}
                </span>
                <button
                  type="button"
                  className="pill-action pill-action--sm browser-page__picker-exit"
                  onClick={() => void setPickerMode(active.id, null)}
                >
                  {t("picker.exit")}
                </button>
              </>
            ) : (
              <span
                className={`browser-page__picker-hint${pickerNotice ? " is-error" : ""}`}
                role="status"
                aria-live="polite"
              >
                {pickerNotice || t("picker.sendHint")}
              </span>
            )}
          </div>
          <div className="browser-page__picker-rows">
            {pickRows.length === 0 && (
              <div className="browser-page__picker-empty">{t("picker.empty")}</div>
            )}
            {pickRows.map((row) => (
              <div key={row.pickId} className="browser-pick-row">
                {row.screenshot ? (
                  <img className="browser-pick-row__thumb" src={row.screenshot} alt="" />
                ) : row.screenshotFailed ? (
                  <button
                    type="button"
                    className="browser-pick-row__thumb is-pending is-failed"
                    title={t("picker.retryScreenshot")}
                    onClick={() => retryScreenshot(active.id, row.pickId)}
                  >
                    {t("picker.retryScreenshot")}
                  </button>
                ) : (
                  <div className="browser-pick-row__thumb is-pending">
                    {t("picker.screenshotPending")}
                  </div>
                )}
                <div className="browser-pick-row__body">
                  <div className="browser-pick-row__title">
                    {row.kind === "element" && row.info ? (
                      <>
                        {pickTargetLabel(row.info)}
                        {(row.info.text || row.info.placeholder) && (
                          <span className="browser-pick-row__text">
                            「{row.info.text || row.info.placeholder}」
                          </span>
                        )}
                      </>
                    ) : (
                      <>
                        {t("picker.modeRegion")}
                        {row.rect && (
                          <span className="browser-pick-row__text">
                            {" "}
                            {Math.round(row.rect.width)}×{Math.round(row.rect.height)}
                          </span>
                        )}
                        {row.scanCapped && (
                          <span className="browser-pick-row__capped">{t("picker.regionCapped")}</span>
                        )}
                      </>
                    )}
                  </div>
                  {row.kind === "region" && row.elements && row.elements.length > 0 && (
                    <div className="browser-pick-row__elements">
                      {row.elements.slice(0, 5).map(elementShort).join(" / ")}
                      {row.elements.length > 5 ? " / …" : ""}
                    </div>
                  )}
                  <input
                    className="browser-pick-row__note"
                    value={row.note}
                    placeholder={t("picker.notePlaceholder")}
                    spellCheck={false}
                    onChange={(event) => {
                      const { value } = event.target;
                      setPicksByTab((prev) => ({
                        ...prev,
                        [active.id]: (prev[active.id] ?? []).map((existing) =>
                          existing.pickId === row.pickId ? { ...existing, note: value } : existing,
                        ),
                      }));
                    }}
                  />
                </div>
                <button
                  type="button"
                  className="pill-action pill-action--icon pill-action--sm browser-pick-row__remove"
                  title={t("picker.remove")}
                  aria-label={t("picker.remove")}
                  onClick={() => removePick(row.pickId)}
                >
                  <X size={11} />
                </button>
              </div>
            ))}
          </div>
          <div className="browser-page__picker-foot">
            <button
              type="button"
              className="pill-action pill-action--sm"
              disabled={pickRows.length === 0 || resolving}
              onClick={() => void clearBatch()}
            >
              {t("picker.clear")}
            </button>
            <button
              type="button"
              className="pill-action pill-action--sm browser-page__picker-send"
              disabled={pickRows.length === 0 || resolving || pendingScreenshots || !onSendToChat}
              title={pendingScreenshots ? t("picker.waitScreenshots") : t("picker.sendHint")}
              onClick={() => void sendToChat()}
            >
              {resolving
                ? t("picker.resolving")
                : t("picker.send", { total: pickRows.length })}
            </button>
          </div>
        </div>
      )}
      <div ref={hostRef} className="browser-page__host">
        {/* Native webviews attach here (positioned by the Rust side). */}
        {tabs.length === 0 && <div className="browser-page__empty">{t("browser.empty")}</div>}
      </div>
    </div>
  );
}

/**
 * The permission card the Rust tool channel raises for Control-level actions
 * (click/type/press/select). 120s of silence denies on the Rust side; this
 * dialog is the answering end. "Remember" also flips the tab's grant, so the
 * next action on this origin does not ask again.
 */
export function BrowserPermissionDialog({
  request,
  sessionLabel,
  onClose,
}: {
  request: BrowserPermissionRequest;
  sessionLabel: string | null;
  onClose: (decision: "allow" | "remember" | "deny") => void;
}) {
  const t = useT();
  return (
    <div className="project-dialog-backdrop" role="presentation">
      <div className="browser-permission" role="dialog" aria-modal>
        <div className="browser-permission__title">{t("browser.permissionTitle")}</div>
        {sessionLabel && <div className="browser-permission__session">{sessionLabel}</div>}
        <div className="browser-permission__body">
          {t("browser.permissionBody", { origin: request.origin, action: request.summary })}
        </div>
        <div className="browser-permission__actions">
          <button type="button" className="project-dialog__cancel" onClick={() => onClose("deny")}>
            {t("browser.permissionDeny")}
          </button>
          <button type="button" className="project-dialog__submit" onClick={() => onClose("allow")}>
            {t("browser.permissionAllow")}
          </button>
          <button type="button" className="project-dialog__submit" onClick={() => onClose("remember")}>
            {t("browser.permissionRemember")}
          </button>
        </div>
      </div>
    </div>
  );
}
