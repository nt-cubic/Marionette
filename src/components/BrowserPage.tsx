import { useEffect, useRef, useState } from "react";
import { Bot, Globe, Plus, RefreshCw, Share2, X } from "lucide-react";
import {
  browserCloseTab,
  browserNavigateTab,
  browserOpenTab,
  browserSetTabBounds,
  browserSetTabVisible,
  browserShareTab,
  type BrowserPermissionRequest,
  type BrowserTab,
} from "../lib/browserTabs";
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
};

export function BrowserPage({ tabs, activeTabId, onSelectTab, visible, onExit }: BrowserPageProps) {
  const t = useT();
  const hostRef = useRef<HTMLDivElement>(null);
  const [address, setAddress] = useState("");
  const [shareMenuFor, setShareMenuFor] = useState<string | null>(null);
  const active = tabs.find((tab) => tab.id === activeTabId) ?? tabs[0] ?? null;

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
  // expect.
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

  // Esc leaves the page (a share menu open consumes it first). The page is
  // mounted only while active, so a window-level listener is safe.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (shareMenuFor) {
          setShareMenuFor(null);
          return;
        }
        onExit?.();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onExit, shareMenuFor]);

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
