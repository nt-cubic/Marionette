import { useCallback, useEffect, useRef, useState } from "react";
import { Bot, Crosshair, Globe, Plus, RefreshCw, Share2, X } from "lucide-react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  browserCloseTab,
  browserNavigateTab,
  browserOpenTab,
  browserPickFinalize,
  browserPickResolve,
  browserPickSet,
  browserSetTabBounds,
  browserSetTabVisible,
  browserShareTab,
  onBrowserPick,
  type BrowserPickEvent,
  type BrowserPickInfo,
  type BrowserPickMode,
  type BrowserPermissionRequest,
  type BrowserTab,
} from "../lib/browserTabs";
import {
  composePickPrompt,
  elementShort,
  pickTargetLabel,
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
  /** Esc: leave the page (tabs stay open; the dock mount maps it to 返回). */
  onExit?: () => void;
  /** 批注抽屉的「发给对话」：prompt 进 composer 预填，图随附件管线落盘。 */
  onSendToChat?: (prompt: string, images: string[]) => void;
};

/** One annotation row: what the page reported plus the user's note. */
type PickRow = {
  pickId: string;
  kind: "element" | "region";
  url: string;
  info?: BrowserPickInfo;
  rect?: { x: number; y: number; width: number; height: number };
  elements?: BrowserPickInfo[];
  scanCapped?: boolean;
  note: string;
  screenshot: string | null;
  screenshotFailed?: boolean;
};

const FINALIZE_RETRIES = 6;
const FINALIZE_RETRY_MS = 400;

export function BrowserPage({ tabs, activeTabId, onSelectTab, visible, onExit, onSendToChat }: BrowserPageProps) {
  const t = useT();
  const hostRef = useRef<HTMLDivElement>(null);
  const [address, setAddress] = useState("");
  const [shareMenuFor, setShareMenuFor] = useState<string | null>(null);
  const active = tabs.find((tab) => tab.id === activeTabId) ?? tabs[0] ?? null;
  // Pick state lives per tab: mode while the page picker runs, and the rows
  // collected so far. A navigation stops the page picker for that tab (the
  // backend resets it) and the drawer stays to let the user finish sending.
  const [pickModes, setPickModes] = useState<Record<string, BrowserPickMode | null>>({});
  const [picksByTab, setPicksByTab] = useState<Record<string, PickRow[]>>({});
  const [resolving, setResolving] = useState(false);
  const pickerMode = active ? (pickModes[active.id] ?? null) : null;
  const pickRows = active ? (picksByTab[active.id] ?? []) : [];
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;

  // Address bar follows the active tab; typing is a draft, Enter navigates.
  useEffect(() => {
    setAddress(active?.url ?? "");
  }, [active?.id, active?.url]);

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

  const setPickerMode = useCallback((tabId: string, mode: BrowserPickMode | null) => {
    setPickModes((prev) => (prev[tabId] === mode ? prev : { ...prev, [tabId]: mode }));
    void browserPickSet(tabId, mode !== null, mode ?? "element");
  }, []);

  const togglePicker = () => {
    if (!active) return;
    const current = pickModes[active.id] ?? null;
    setPickerMode(active.id, current ? null : "element");
  };

  // A crop can land while another tab's CDP call holds the lease — retry a
  // few times before showing the failed chip.
  const finalizeScreenshot = useCallback(async (tabId: string, pickId: string) => {
    for (let attempt = 0; attempt < FINALIZE_RETRIES; attempt++) {
      const result = await browserPickFinalize(tabId, pickId);
      if (result) {
        setPicksByTab((prev) => ({
          ...prev,
          [tabId]: (prev[tabId] ?? []).map((row) =>
            row.pickId === pickId ? { ...row, screenshot: result.dataUrl } : row,
          ),
        }));
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, FINALIZE_RETRY_MS));
    }
    setPicksByTab((prev) => ({
      ...prev,
      [tabId]: (prev[tabId] ?? []).map((row) =>
        row.pickId === pickId ? { ...row, screenshotFailed: true } : row,
      ),
    }));
  }, []);

  // Picks travel page → Rust event → drawer; the crop (and everything else
  // CDP) is frontend-driven from here.
  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;
    void onBrowserPick((event: BrowserPickEvent) => {
      const { tabId, message } = event;
      if (message.kind === "cancel") {
        setPickModes((prev) => (prev[tabId] ? { ...prev, [tabId]: null } : prev));
        return;
      }
      const url = tabsRef.current.find((tab) => tab.id === tabId)?.url ?? "";
      const row: PickRow =
        message.kind === "pick"
          ? { pickId: message.pickId, kind: "element", url, info: message.info, note: "", screenshot: null }
          : {
              pickId: message.pickId,
              kind: "region",
              url,
              rect: message.rect,
              elements: message.elements,
              scanCapped: message.scanCapped === true,
              note: "",
              screenshot: null,
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
  }, [finalizeScreenshot]);

  // A navigation stops the page picker on the backend; mirror that here so
  // the crosshair stops glowing on a page that no longer picks.
  const urlsRef = useRef<Record<string, string>>({});
  useEffect(() => {
    for (const tab of tabs) {
      if (urlsRef.current[tab.id] !== undefined && urlsRef.current[tab.id] !== tab.url) {
        setPickModes((prev) => (prev[tab.id] ? { ...prev, [tab.id]: null } : prev));
      }
      urlsRef.current[tab.id] = tab.url;
    }
  }, [tabs]);

  const sendToChat = async () => {
    const tab = active;
    if (!tab || !onSendToChat || resolving) return;
    const rows = picksByTab[tab.id] ?? [];
    if (rows.length === 0) return;
    setResolving(true);
    try {
      const resolved = await browserPickResolve(tab.id);
      const refs: BrowserPickRefs = resolved?.refs ?? {};
      const generation = resolved?.generation ?? "";
      const withShot = rows.filter((row) => row.screenshot);
      const prompt = composePickPrompt(rows, refs, generation, withShot.map((row) => row.pickId));
      onSendToChat(prompt, withShot.map((row) => row.screenshot as string));
      setPicksByTab((prev) => ({ ...prev, [tab.id]: [] }));
      setPickerMode(tab.id, null);
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

  // Esc leaves the page (a share menu consumes it first, then the picker).
  // The page picker has its own Esc path — the in-page script sends a cancel
  // event that resets `pickModes` — so this only fires when the main window
  // holds focus (address bar, drawer inputs).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (shareMenuFor) {
          setShareMenuFor(null);
          return;
        }
        if (active && (pickModes[active.id] ?? null)) {
          setPickerMode(active.id, null);
          return;
        }
        onExit?.();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, pickModes, setPickerMode, onExit, shareMenuFor]);

  const drawerOpen = Boolean(active) && (pickerMode !== null || pickRows.length > 0);

  return (
    <div className="browser-page">
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
                      className={`browser-page__picker-mode${pickerMode === mode ? " is-active" : ""}`}
                      onClick={() => setPickerMode(active.id, mode)}
                    >
                      {t(mode === "element" ? "picker.modeElement" : "picker.modeRegion")}
                    </button>
                  ))}
                </div>
                <span className="browser-page__picker-hint">
                  {t(pickerMode === "element" ? "picker.hint" : "picker.hintRegion")}
                </span>
                <button
                  type="button"
                  className="pill-action pill-action--sm browser-page__picker-exit"
                  onClick={() => setPickerMode(active.id, null)}
                >
                  {t("picker.exit")}
                </button>
              </>
            ) : (
              <span className="browser-page__picker-hint">{t("picker.sendHint")}</span>
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
                ) : (
                  <div className="browser-pick-row__thumb is-pending">
                    {row.screenshotFailed ? t("picker.screenshotFailed") : ""}
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
                  title={t("browser.closeTab")}
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
              disabled={pickRows.length === 0}
              onClick={() => active && setPicksByTab((prev) => ({ ...prev, [active.id]: [] }))}
            >
              {t("picker.clear")}
            </button>
            <button
              type="button"
              className="pill-action pill-action--sm browser-page__picker-send"
              disabled={pickRows.length === 0 || resolving || !onSendToChat}
              title={t("picker.sendHint")}
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
