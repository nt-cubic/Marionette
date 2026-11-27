import { useCallback, useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import {
  Bot,
  Check,
  ChevronDown,
  ChevronUp,
  Globe,
  MonitorSmartphone,
  MoreHorizontal,
  Pin,
  Plus,
  RefreshCw,
  RotateCw,
  Search,
  Share2,
  UserRound,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  browserCloseTab,
  browserClaimTab,
  browserFind,
  browserNavigateTab,
  browserOpenTab,
  browserPickClear,
  browserPickFinalize,
  browserPickResolve,
  browserPickSet,
  browserSetTabBounds,
  browserSetTabVisible,
  browserShareTab,
  browserSetTabPinned,
  browserViewportReset,
  browserViewportSet,
  onBrowserPick,
  type BrowserFindResult,
  type BrowserPickEvent,
  type BrowserPickMode,
  type BrowserHandoffRequest,
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
import { browserHostCovered } from "../lib/browserOverlay";

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
  /** A human-only step the current Agent is paused on. */
  handoff?: BrowserHandoffRequest | null;
  onHandoffRespond?: (
    requestId: string,
    outcome: "continue" | "denied",
  ) => Promise<boolean>;
  ownerSessionLabel?: (sessionId: string) => string;
};

const FINALIZE_RETRIES = 6;
const FINALIZE_RETRY_MS = 400;
const PICKER_TOGGLE_RETRIES = 6;
const PICKER_TOGGLE_RETRY_MS = 350;

// Responsive preview presets. `width`/`height` 0 means "the host's own size"
// (the desktop preset); phone/tablet are the CSS px of the emulated viewport.
// These only change what the preview renders (Emulation.setDeviceMetricsOverride)
// and are reset when the tab changes or this page unmounts.
type ViewportPreset = "desktop" | "phone" | "tablet";
const VIEWPORT_PRESETS: Record<
  ViewportPreset,
  { width: number; height: number; mobile: boolean }
> = {
  desktop: { width: 0, height: 0, mobile: false },
  phone: { width: 390, height: 844, mobile: true },
  tablet: { width: 820, height: 1180, mobile: true },
};
const VIEWPORT_ZOOM_MIN = 0.5;
const VIEWPORT_ZOOM_MAX = 2;
const VIEWPORT_ZOOM_STEPS = [0.5, 0.75, 1, 1.25, 1.5, 2] as const;

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
  handoff = null,
  onHandoffRespond,
  ownerSessionLabel,
}: BrowserPageProps) {
  const t = useT();
  const rootRef = useRef<HTMLDivElement>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  const [covered, setCovered] = useState(false);
  const [address, setAddress] = useState("");
  const [shareMenuFor, setShareMenuFor] = useState<string | null>(null);
  const [tabMenuFor, setTabMenuFor] = useState<string | null>(null);
  const active = tabs.find((tab) => tab.id === activeTabId) ?? tabs[0] ?? null;
  const menuTab = tabs.find((tab) => tab.id === tabMenuFor) ?? null;
  const [resolving, setResolving] = useState(false);
  const [pickerNotice, setPickerNotice] = useState<string | null>(null);
  const [handoffBusy, setHandoffBusy] = useState(false);
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

  // -- Find on page -------------------------------------------------------
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [findResult, setFindResult] = useState<
    (BrowserFindResult & { error?: boolean }) | null
  >(null);
  const findInputRef = useRef<HTMLInputElement>(null);
  const findSeq = useRef(0);

  // -- Responsive preview -------------------------------------------------
  const [viewportOpen, setViewportOpen] = useState(false);
  const [viewportPreset, setViewportPreset] = useState<ViewportPreset>("desktop");
  const [viewportRotated, setViewportRotated] = useState(false);
  const [viewportZoom, setViewportZoom] = useState(1);
  const [viewportNotice, setViewportNotice] = useState<string | null>(null);
  const [viewportHostTick, setViewportHostTick] = useState(0);
  const emulatedTabRef = useRef<string | null>(null);
  const emulationActiveRef = useRef(false);
  emulationActiveRef.current =
    viewportPreset !== "desktop" || viewportZoom !== 1;

  // Address bar follows the active tab; typing is a draft, Enter navigates.
  useEffect(() => {
    setAddress(active?.url ?? "");
  }, [active?.id, active?.url]);

  useEffect(() => {
    setPickerNotice(null);
  }, [active?.id]);

  useEffect(() => {
    if (tabMenuFor && !tabs.some((tab) => tab.id === tabMenuFor)) {
      setTabMenuFor(null);
    }
  }, [tabMenuFor, tabs]);

  useEffect(() => {
    setHandoffBusy(false);
  }, [handoff?.requestId]);

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

  // Native child webviews cannot be covered by HTML. Watch app overlays and
  // temporarily hide the surface until the last covering overlay is gone.
  useEffect(() => {
    if (!visible) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      if (hostRef.current) setCovered(browserHostCovered(hostRef.current));
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    const observer = new MutationObserver(schedule);
    observer.observe(document.body, {
      childList: true, subtree: true, attributes: true,
      attributeFilter: ["class", "style", "hidden", "role", "aria-modal"],
    });
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    update();
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      if (frame) cancelAnimationFrame(frame);
    };
  }, [visible]);

  // Surface visibility: the active tab shows when the page shows; everything
  // else hides. A tab is only made visible AFTER its bounds landed — the
  // native surface starts as a 1×1 dot and a failed measurement must keep it
  // that way, never show it over the app chrome (the 2026-10-31 freeze).
  const shownRef = useRef<string | null>(null);
  useEffect(() => {
    const previous = shownRef.current;
    const target = visible && !covered ? (active?.id ?? null) : null;
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
          if (!cancelled && hostRef.current && !browserHostCovered(hostRef.current)) {
            void browserSetTabVisible(target, true);
          }
          return;
        }
        await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [active?.id, visible, covered]);

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
      // Keep the preview's fit scale honest when the pane is resized.
      if (emulationActiveRef.current) setViewportHostTick((value) => value + 1);
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
    if (tabMenuFor === tabId) setTabMenuFor(null);
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

  const respondToHandoff = async (outcome: "continue" | "denied") => {
    if (!handoff || !onHandoffRespond || handoffBusy) return;
    setHandoffBusy(true);
    try {
      await onHandoffRespond(handoff.requestId, outcome);
    } finally {
      setHandoffBusy(false);
    }
  };

  // -- Find on page -------------------------------------------------------

  const closeFind = useCallback(() => {
    findSeq.current += 1;
    setFindOpen(false);
    setFindQuery("");
    setFindResult(null);
    const tabId = active?.id;
    if (tabId) void browserFind(tabId, "clear");
  }, [active?.id]);

  const stepFind = useCallback(
    async (backwards: boolean) => {
      const tabId = active?.id;
      if (!tabId || !findQuery.trim() || !findResult || findResult.count === 0) return;
      const seq = ++findSeq.current;
      const result = await browserFind(tabId, backwards ? "prev" : "next");
      if (findSeq.current !== seq) return;
      setFindResult(result ?? { count: 0, index: -1, error: true });
    },
    [active?.id, findQuery, findResult],
  );

  // Which Ctrl F is this? The browser surface owns the key when it is the
  // focused pane (or the dock's full-page browser), not when a session pane
  // beside it holds focus. The native page itself keeps the key: keyboard
  // events inside the child WebView2 do not reach this webview (manual gap).
  const browserOwnsFind = useCallback(() => {
    if (!visible) return false;
    const root = rootRef.current;
    if (!root) return false;
    if (root.closest(".browser-dock-page")) return true;
    const activeEl = document.activeElement;
    if (activeEl && root.contains(activeEl)) return true;
    const pane = root.closest(".session-pane");
    if (pane) return pane.classList.contains("is-focused");
    return !document.querySelector(".session-pane.is-focused");
  }, [visible]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.shiftKey || event.altKey) return;
      if (event.key !== "f" && event.key !== "F") return;
      if (!browserOwnsFind()) return;
      // App.tsx opens the conversation find on Ctrl F; a focused browser
      // surface owns it instead. Capture + stopImmediatePropagation keeps the
      // transcript listener from also firing.
      event.preventDefault();
      event.stopImmediatePropagation();
      setFindOpen(true);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [browserOwnsFind]);

  // Focus the field when the bar opens.
  useEffect(() => {
    if (!findOpen) return;
    const input = findInputRef.current;
    if (input) {
      input.focus();
      input.select();
    }
  }, [findOpen]);

  // Search as you type. A sequence guard drops answers that arrive after a
  // newer query. Navigation (url change) re-runs the same query.
  useEffect(() => {
    if (!findOpen) return;
    const tabId = active?.id;
    if (!tabId) return;
    const seq = ++findSeq.current;
    const query = findQuery;
    setFindResult(null);
    if (!query.trim()) {
      void browserFind(tabId, "clear");
      return;
    }
    const timer = window.setTimeout(() => {
      void browserFind(tabId, "search", query, false).then((result) => {
        if (findSeq.current !== seq) return;
        setFindResult(result ?? { count: 0, index: -1, error: true });
      });
    }, 120);
    return () => window.clearTimeout(timer);
  }, [active?.id, active?.url, findOpen, findQuery]);

  // Leaving the tab drops the page selection and closes the bar: those ranges
  // described a document the user is no longer looking at.
  const findTabRef = useRef<string | null>(null);
  useEffect(() => {
    const tabId = active?.id ?? null;
    const previous = findTabRef.current;
    findTabRef.current = tabId;
    if (previous && previous !== tabId) {
      findSeq.current += 1;
      void browserFind(previous, "clear");
      setFindOpen(false);
      setFindQuery("");
      setFindResult(null);
    }
  }, [active?.id]);

  // -- Responsive preview -------------------------------------------------
  // Presentation only: desktop at 100% clears the emulation; anything else
  // renders the page at an emulated viewport with a fit scale. The patch,
  // the page's data and the browser's real size are never changed.

  const zoomBy = (direction: 1 | -1) => {
    setViewportZoom((current) => {
      const at = VIEWPORT_ZOOM_STEPS.findIndex((step) => step >= current - 0.001);
      const index = Math.min(
        VIEWPORT_ZOOM_STEPS.length - 1,
        Math.max(0, (at < 0 ? 0 : at) + direction),
      );
      return VIEWPORT_ZOOM_STEPS[index] ?? VIEWPORT_ZOOM_MAX;
    });
  };

  useEffect(() => {
    const tab = active;
    if (!tab || !visible) return;
    let cancelled = false;
    const previous = emulatedTabRef.current;
    if (previous && previous !== tab.id) {
      emulatedTabRef.current = null;
      void browserViewportReset(previous);
    }
    const preset = VIEWPORT_PRESETS[viewportPreset];
    const wantsEmulation = viewportPreset !== "desktop" || viewportZoom !== 1;
    if (!wantsEmulation) {
      emulatedTabRef.current = null;
      void browserViewportReset(tab.id).then((ok) => {
        if (!cancelled) setViewportNotice(ok ? null : t("browser.viewportFailed"));
      });
      return () => {
        cancelled = true;
      };
    }
    const rect = hostRef.current?.getBoundingClientRect();
    const hostW = Math.max(240, Math.round(rect?.width || window.innerWidth));
    const hostH = Math.max(240, Math.round(rect?.height || window.innerHeight));
    let width = preset.width || hostW;
    let height = preset.height || hostH;
    if (preset.width > 0 && viewportRotated) {
      const swap = width;
      width = height;
      height = swap;
    }
    const fit = Math.min(1, hostW / width, hostH / height);
    const scale =
      Math.round(Math.min(2, Math.max(0.25, fit * viewportZoom)) * 100) / 100;
    emulatedTabRef.current = tab.id;
    void browserViewportSet(tab.id, width, height, preset.mobile, scale).then(
      (ok) => {
        if (!cancelled) setViewportNotice(ok ? null : t("browser.viewportFailed"));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [
    active?.id,
    active?.url,
    visible,
    viewportPreset,
    viewportRotated,
    viewportZoom,
    viewportHostTick,
    t,
  ]);

  // A native surface outlives this React page; drop the emulation on unmount.
  useEffect(
    () => () => {
      if (emulatedTabRef.current) {
        void browserViewportReset(emulatedTabRef.current);
        emulatedTabRef.current = null;
      }
    },
    [],
  );

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
        if (findOpen) {
          closeFind();
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
  }, [active, closeFind, embedded, findOpen, pickModes, setPickerMode, onExit, shareMenuFor]);

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
                title={[
                  tab.chosenId ? t("browser.chosenTab", { id: tab.chosenId }) : null,
                  tab.title || tab.url,
                  tab.url,
                ]
                  .filter(Boolean)
                  .join(" · ")}
                onClick={() => {
                  setTabMenuFor(null);
                  onSelectTab(tab.id);
                }}
              >
                <Globe size={11} />
                <span>{tab.chosenId || tab.title || tab.origin || tab.url}</span>
                {tab.pinned && (
                  <i className="browser-tab__pinned" title={t("browser.pinned")}>
                    <Pin size={10} />
                  </i>
                )}
                {tab.openedByAgent && (
                  <i
                    className="browser-tab__agent"
                    title={
                      tab.ownerSessionId
                        ? t("browser.ownerSession", {
                            id:
                              ownerSessionLabel?.(tab.ownerSessionId) ??
                              tab.ownerSessionId,
                          })
                        : t("browser.openedByAgent")
                    }
                  >
                    <Bot size={10} />
                  </i>
                )}
                {tab.status !== "idle" && (
                  <small
                    className={`browser-tab__status is-${tab.status}`}
                    title={t(`browser.status.${tab.status}` as never)}
                  >
                    {t(("browser.statusShort." + tab.status) as never)}
                  </small>
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
                className="pill-action pill-action--icon pill-action--sm browser-tab__more"
                title={t("browser.tabActions")}
                aria-label={t("browser.tabActions")}
                aria-expanded={tabMenuFor === tab.id}
                onClick={() =>
                  setTabMenuFor((current) => (current === tab.id ? null : tab.id))
                }
              >
                <MoreHorizontal size={11} />
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
        {menuTab && (
          <div className="browser-tab-actions" role="menu">
            <div className="browser-tab-actions__identity">
              <b>{menuTab.chosenId || menuTab.title || menuTab.url}</b>
              <span>
                {menuTab.ownerSessionId
                  ? t("browser.ownerSession", {
                      id:
                        ownerSessionLabel?.(menuTab.ownerSessionId) ??
                        menuTab.ownerSessionId,
                    })
                  : t("browser.userOwned")}
              </span>
            </div>
            <button
              type="button"
              role="menuitem"
              className={menuTab.pinned ? "is-active" : ""}
              onClick={() => {
                void browserSetTabPinned(menuTab.id, !menuTab.pinned).catch(
                  () => undefined,
                );
                setTabMenuFor(null);
              }}
            >
              <Pin size={12} />
              {menuTab.pinned ? t("browser.unpin") : t("browser.pin")}
            </button>
            {menuTab.openedByAgent && (
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  void browserClaimTab(menuTab.id).catch(() => undefined);
                  setTabMenuFor(null);
                }}
              >
                <UserRound size={12} />
                {t("browser.claim")}
              </button>
            )}
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setTabMenuFor(null);
                closeTab(menuTab.id);
              }}
            >
              <X size={12} />
              {t("browser.closeTab")}
            </button>
          </div>
        )}
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
              className={`pill-action pill-action--icon pill-action--sm${
                findOpen ? " is-active" : ""
              }`}
              title={t("browser.findPlaceholder")}
              aria-label={t("browser.findPlaceholder")}
              aria-pressed={findOpen}
              onClick={() => (findOpen ? closeFind() : setFindOpen(true))}
            >
              <Search size={12} />
            </button>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              title={t("browser.reload")}
              onClick={() => void browserNavigateTab(active.id, active.url)}
            >
              <RefreshCw size={12} />
            </button>
            {(viewportPreset !== "desktop" || viewportZoom !== 1) && (
              <span className="browser-page__emulated" title={t("browser.viewportNote")}>
                {t("browser.viewportBadge")}
              </span>
            )}
            <button
              type="button"
              className={`pill-action pill-action--icon pill-action--sm browser-page__viewport-btn${
                viewportOpen || viewportPreset !== "desktop" || viewportZoom !== 1
                  ? " is-active"
                  : ""
              }`}
              title={t("browser.viewport")}
              aria-label={t("browser.viewport")}
              aria-expanded={viewportOpen}
              onClick={() => setViewportOpen((value) => !value)}
            >
              <MonitorSmartphone size={12} />
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
        {active && findOpen && (
          <div className="find-bar find-bar--inline browser-page__find" role="search">
            <Search size={12} aria-hidden />
            <input
              ref={findInputRef}
              className="find-bar__input"
              type="text"
              value={findQuery}
              placeholder={t("browser.findPlaceholder")}
              aria-label={t("browser.findPlaceholder")}
              spellCheck={false}
              onChange={(event) => setFindQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  event.stopPropagation();
                  closeFind();
                } else if (event.key === "Enter") {
                  event.preventDefault();
                  event.stopPropagation();
                  void stepFind(event.shiftKey);
                }
              }}
            />
            <span
              className={`find-bar__count${
                findResult?.error ||
                (findQuery.trim() && (findResult?.count ?? 0) === 0)
                  ? " is-empty"
                  : ""
              }`}
              title={findResult?.truncated ? t("browser.findTruncated") : undefined}
            >
              {findResult?.error
                ? t("browser.findFailed")
                : findResult && findResult.count > 0
                  ? t("browser.findCounter", {
                      i: findResult.index + 1,
                      n: `${findResult.count}${findResult.truncated ? "+" : ""}`,
                    })
                  : findQuery.trim()
                    ? t("browser.findNoMatch")
                    : ""}
            </span>
            <button
              type="button"
              className="find-bar__btn"
              title={t("browser.findPrev")}
              aria-label={t("browser.findPrev")}
              disabled={!findResult || findResult.count === 0}
              onClick={() => void stepFind(true)}
            >
              <ChevronUp size={12} aria-hidden />
            </button>
            <button
              type="button"
              className="find-bar__btn"
              title={t("browser.findNext")}
              aria-label={t("browser.findNext")}
              disabled={!findResult || findResult.count === 0}
              onClick={() => void stepFind(false)}
            >
              <ChevronDown size={12} aria-hidden />
            </button>
            <button
              type="button"
              className="find-bar__btn"
              title={t("browser.findClose")}
              aria-label={t("browser.findClose")}
              onClick={closeFind}
            >
              <X size={12} aria-hidden />
            </button>
          </div>
        )}
        {active && viewportOpen && (
          <div className="browser-viewport" role="group" aria-label={t("browser.viewport")}>
            <div className="browser-viewport__head">
              <span className="browser-viewport__title">{t("browser.viewport")}</span>
              <small className="browser-viewport__note">{t("browser.viewportNote")}</small>
            </div>
            <div className="browser-viewport__row">
              {(["desktop", "phone", "tablet"] as const).map((preset) => (
                <button
                  key={preset}
                  type="button"
                  className={`browser-viewport__chip${
                    viewportPreset === preset ? " is-active" : ""
                  }`}
                  aria-pressed={viewportPreset === preset}
                  onClick={() => {
                    setViewportPreset(preset);
                    if (preset === "desktop") setViewportRotated(false);
                  }}
                >
                  {t(("browser.viewportPreset." + preset) as never)}
                </button>
              ))}
              <button
                type="button"
                className="browser-viewport__icon"
                title={t("browser.viewportRotate")}
                aria-label={t("browser.viewportRotate")}
                aria-pressed={viewportRotated}
                disabled={viewportPreset === "desktop"}
                onClick={() => setViewportRotated((value) => !value)}
              >
                <RotateCw size={12} />
              </button>
            </div>
            <div className="browser-viewport__row">
              <span className="browser-viewport__label">{t("browser.viewportZoom")}</span>
              <button
                type="button"
                className="browser-viewport__icon"
                title={t("browser.viewportZoomOut")}
                aria-label={t("browser.viewportZoomOut")}
                disabled={viewportZoom <= VIEWPORT_ZOOM_MIN}
                onClick={() => zoomBy(-1)}
              >
                <ZoomOut size={12} />
              </button>
              <span className="browser-viewport__zoom">{Math.round(viewportZoom * 100)}%</span>
              <button
                type="button"
                className="browser-viewport__icon"
                title={t("browser.viewportZoomIn")}
                aria-label={t("browser.viewportZoomIn")}
                disabled={viewportZoom >= VIEWPORT_ZOOM_MAX}
                onClick={() => zoomBy(1)}
              >
                <ZoomIn size={12} />
              </button>
              <button
                type="button"
                className="browser-viewport__reset"
                onClick={() => setViewportZoom(1)}
              >
                {t("browser.viewportZoomReset")}
              </button>
              {viewportNotice && (
                <span className="browser-viewport__notice" role="status">
                  {viewportNotice}
                </span>
              )}
            </div>
          </div>
        )}
      </div>
      {active && (
        <div className="browser-page__picker">
          <div className="browser-page__picker-head">
            <div className="browser-page__picker-modes">
              {(["element", "region"] as const).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  aria-pressed={pickerMode === mode}
                  className={`browser-page__picker-mode${pickerMode === mode ? " is-active" : ""}`}
                  onClick={() => void setPickerMode(active.id, pickerMode === mode ? null : mode)}
                >
                  {t(mode === "element" ? "picker.modeElement" : "picker.modeRegion")}
                </button>
              ))}
            </div>
            {(pickerMode || pickerNotice) && <span
              className={`browser-page__picker-hint${pickerNotice ? " is-error" : ""}`}
              role="status"
              aria-live="polite"
            >
              {pickerNotice ||
                t(pickerMode === "element" ? "picker.hint" : "picker.hintRegion")}
            </span>}
            {pickerMode && <button
              type="button"
              className="pill-action pill-action--sm browser-page__picker-exit"
              onClick={() => void setPickerMode(active.id, null)}
            >
              {t("picker.exit")}
            </button>}
          </div>
          {pickRows.length > 0 && (
            <>
              <div className="browser-page__picker-rows">
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
            </>
          )}
        </div>
      )}
      {handoff && active?.id === handoff.tabId && (
        <div className="browser-page__handoff" role="status" aria-live="polite">
          <div className="browser-page__handoff-copy">
            <b>{handoff.title}</b>
            <span>{handoff.message}</span>
            <small>{handoff.origin}</small>
          </div>
          <div className="browser-page__handoff-actions">
            <button
              type="button"
              className="pill-action pill-action--sm"
              disabled={handoffBusy}
              onClick={() => void respondToHandoff("denied")}
            >
              {t("browser.handoffDeny")}
            </button>
            <button
              type="button"
              className="pill-action pill-action--sm browser-page__handoff-continue"
              disabled={handoffBusy}
              onClick={() => void respondToHandoff("continue")}
            >
              <Check size={12} />
              {t("browser.handoffContinue")}
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
