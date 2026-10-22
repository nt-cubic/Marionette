/**
 * OrbApp — root of the floating-orb window (`index.html?orb=1`, label "orb").
 *
 * The orb is a remote control: it renders the ball / panel UI and emits
 * OrbCommand events; the (possibly hidden) main window executes them and
 * pushes OrbState back. This component owns:
 *
 *  - circle ⇄ panel mode transitions (resizeOrbWindow keeps the bottom-right
 *    corner anchored);
 *  - the capture pipeline (HTML5 drag-and-drop + paste → capture_save_blob /
 *    capture_fetch_url → OrbCommand "capture");
 *  - notice sounds (only while the main window is hidden, dedup by `at`);
 *  - collapse triggers (window blur after a grace period, Escape) — never
 *    while an HTML5 drag is hovering.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { engineQuota, isTauriRuntime } from "../lib/api";
import {
  emitOrbCommand,
  listenOrbState,
  type OrbNoticeKind,
  type OrbState,
} from "../lib/orbEvents";
import {
  applySavedOrbPosition,
  bindOrbMovePersistence,
  resizeOrbWindow,
} from "../lib/orbWindow";
import { BUILTIN_SCENARIOS, listScenarios, type Scenario } from "../lib/scenarios";
import completeWav from "../assets/notifications/complete.wav";
import errorWav from "../assets/notifications/error.wav";
import permissionWav from "../assets/notifications/permission.wav";
import { OrbBall, worstNoticeKind } from "./OrbBall";
import { OrbPanel, type OrbToast } from "./OrbPanel";
import type { OrbQuotaSnapshot } from "./OrbQuota";
import {
  buildCapturePayload,
  extractCaptureItems,
  isCaptureEmpty,
  isCapturePayloadEmpty,
  type RawCaptureItems,
} from "./orbCapture";

type OrbMode = "circle" | "panel";

const IDLE_ORB_STATE: OrbState = { mainVisible: true, running: [], notices: [] };

/** Mirrors desktopNotify.ts: import the bundled WAV, play best-effort. */
const NOTICE_SOUNDS: Record<OrbNoticeKind, string> = {
  reply: completeWav,
  permission: permissionWav,
  error: errorWav,
};

function playNoticeSound(kind: OrbNoticeKind): void {
  try {
    const audio = new Audio(NOTICE_SOUNDS[kind]);
    audio.preload = "auto";
    window.setTimeout(() => {
      try {
        audio.pause();
      } catch {
        /* ignore */
      }
    }, 5000);
    void audio.play().catch(() => undefined);
  } catch {
    /* autoplay policy / missing asset — badge stays the primary channel */
  }
}

export function OrbApp() {
  const [mode, setMode] = useState<OrbMode>("circle");
  const [orbState, setOrbState] = useState<OrbState>(IDLE_ORB_STATE);
  const [scenarios, setScenarios] = useState<Scenario[]>(BUILTIN_SCENARIOS);
  const [quickText, setQuickText] = useState("");
  const [dragHover, setDragHover] = useState(false);
  const [processingCount, setProcessingCount] = useState(0);
  const [toasts, setToasts] = useState<OrbToast[]>([]);
  const [quota, setQuota] = useState<OrbQuotaSnapshot | null>(null);
  const [quotaBusy, setQuotaBusy] = useState(false);
  /** Re-rendered every second while tasks run (elapsed timers). */
  const [nowMs, setNowMs] = useState(() => Date.now());

  const modeRef = useRef<OrbMode>("circle");
  /** True while an HTML5 drag is hovering — suppresses auto-collapse. */
  const dragActiveRef = useRef(false);
  /** Notice timestamps already seen (sound dedup; acked by clearNotices). */
  const seenNoticeAtRef = useRef<Set<number>>(new Set());
  const firstStateRef = useRef(true);
  const toastIdRef = useRef(0);
  const quickInputRef = useRef<HTMLInputElement>(null);

  const pushToast = useCallback(
    (text: string, tone: OrbToast["tone"] = "ok", ttlMs = tone === "ok" ? 2000 : 4000) => {
      const id = ++toastIdRef.current;
      setToasts((prev) => [...prev.slice(-2), { id, text, tone }]);
      window.setTimeout(() => {
        setToasts((prev) => prev.filter((t) => t.id !== id));
      }, ttlMs);
    },
    []
  );

  const expand = useCallback(() => {
    modeRef.current = "panel";
    setMode("panel");
    void resizeOrbWindow("panel");
    // Deferred: keep native window calls away from the click paint, and let
    // the resize settle before the input grabs focus.
    window.setTimeout(() => {
      try {
        if (isTauriRuntime()) void getCurrentWindow().setFocus();
      } catch {
        /* optional */
      }
      quickInputRef.current?.focus();
    }, 80);
    void emitOrbCommand({ kind: "clearNotices" });
  }, []);

  const collapse = useCallback(() => {
    modeRef.current = "circle";
    setMode("circle");
    void resizeOrbWindow("circle");
  }, []);

  // ── Mount: theme, saved position, move persistence ──────────────────────
  useEffect(() => {
    // Mirror the main window's persisted theme (only "white" overrides tokens).
    try {
      const stored =
        window.localStorage.getItem("marionette-theme") ??
        window.localStorage.getItem("agentshell-theme");
      if (stored === "white") document.documentElement.dataset.theme = "white";
      else document.documentElement.removeAttribute("data-theme");
    } catch {
      /* private mode */
    }
    void applySavedOrbPosition();
    let disposed = false;
    let unbindMoves: (() => void) | undefined;
    void bindOrbMovePersistence().then((unbind) => {
      if (disposed) unbind();
      else unbindMoves = unbind;
    });
    return () => {
      disposed = true;
      unbindMoves?.();
    };
  }, []);

  // ── State from the main window + notice sounds ──────────────────────────
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listenOrbState((state) => {
        setOrbState(state);
        const seen = seenNoticeAtRef.current;
        const fresh = state.notices.filter((n) => !seen.has(n.at));
        for (const n of state.notices) seen.add(n.at);
        if (seen.size > 512) {
          // Prune to the live set — acked/old notices never come back anyway.
          seenNoticeAtRef.current = new Set(state.notices.map((n) => n.at));
        }
        // The first push is a sync of pre-existing state — never chime for it.
        if (firstStateRef.current) {
          firstStateRef.current = false;
          return;
        }
        if (state.mainVisible || fresh.length === 0) return;
        for (const n of fresh) playNoticeSound(n.kind);
      });
      if (disposed) unlisten();
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // ── Scenario buttons ─────────────────────────────────────────────────────
  useEffect(() => {
    void listScenarios()
      .then((list) => setScenarios(Array.isArray(list) ? list : BUILTIN_SCENARIOS))
      .catch(() => setScenarios(BUILTIN_SCENARIOS));
  }, []);

  // ── Balances: fetch on mount, auto-refresh every 5 minutes ──────────────
  const refreshQuota = useCallback(async (force: boolean) => {
    setQuotaBusy(true);
    try {
      const entries = await engineQuota(force);
      setQuota({
        entries,
        updatedAt: Date.now(),
        failed: entries.length === 0,
      });
    } catch {
      setQuota((prev) =>
        prev
          ? { ...prev, failed: true }
          : { entries: [], updatedAt: Date.now(), failed: true }
      );
    } finally {
      setQuotaBusy(false);
    }
  }, []);

  useEffect(() => {
    void refreshQuota(true);
    const timer = window.setInterval(() => void refreshQuota(true), 5 * 60_000);
    return () => window.clearInterval(timer);
  }, [refreshQuota]);

  // ── Elapsed ticking while tasks run ──────────────────────────────────────
  useEffect(() => {
    if (orbState.running.length === 0) return;
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [orbState.running.length]);

  // ── Collapse: window blur (grace period) and Escape ─────────────────────
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let timer: number | undefined;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
          if (focused) {
            if (timer !== undefined) {
              window.clearTimeout(timer);
              timer = undefined;
            }
            return;
          }
          if (modeRef.current !== "panel" || timer !== undefined) return;
          // ~400ms grace: cancels on refocus, and defers the native resize
          // past the focus-change paint (tao flush_paint_messages).
          timer = window.setTimeout(() => {
            timer = undefined;
            if (disposed || modeRef.current !== "panel") return;
            if (dragActiveRef.current) return;
            collapse();
          }, 400);
        });
      } catch {
        /* optional in browser preview */
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [collapse]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (modeRef.current !== "panel" || dragActiveRef.current) return;
      collapse();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [collapse]);

  // ── Capture pipeline (shared by drop + paste) ───────────────────────────
  const runCapture = useCallback(
    async (raw: RawCaptureItems) => {
      if (isCaptureEmpty(raw)) {
        pushToast("没有可收集的内容", "warn");
        return;
      }
      const asyncItems = raw.files.length + raw.urls.length;
      if (asyncItems > 0) setProcessingCount((n) => n + asyncItems);
      const payload = await buildCapturePayload(raw, {
        onItemDone: () => setProcessingCount((n) => Math.max(0, n - 1)),
        onItemFailed: (message) => pushToast(message, "warn"),
      });
      if (isCapturePayloadEmpty(payload)) {
        pushToast("没有可收集的内容", "warn");
        return;
      }
      await emitOrbCommand({ kind: "capture", payload });
      pushToast("已送入收集箱 ✓", "ok", 2000);
    },
    [pushToast]
  );

  // ── Paste (when nothing editable is focused) ─────────────────────────────
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable)
      ) {
        return; // let normal paste into the quick input work
      }
      const dt = e.clipboardData;
      if (!dt) return;
      const images: File[] = [];
      try {
        for (const item of Array.from(dt.items ?? [])) {
          if (item.kind === "file" && item.type.startsWith("image/")) {
            const file = item.getAsFile();
            if (file) images.push(file);
          }
        }
      } catch {
        /* clipboard items unavailable */
      }
      let text: string | null = null;
      try {
        text = dt.getData("text/plain") || null;
      } catch {
        /* ignore */
      }
      if (images.length === 0 && !(text ?? "").trim()) return;
      e.preventDefault();
      void runCapture({ files: images, urls: [], html: null, text });
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [runCapture]);

  // ── HTML5 drag & drop (works in both modes) ─────────────────────────────
  const dragHasCapture = (dt: DataTransfer | null): boolean => {
    if (!dt) return false;
    const types = Array.from(dt.types ?? []);
    return (
      types.includes("Files") ||
      types.includes("text/uri-list") ||
      types.includes("text/plain") ||
      types.includes("text/html")
    );
  };

  const onDragEnter = (e: React.DragEvent<HTMLDivElement>) => {
    if (!dragHasCapture(e.dataTransfer)) return;
    e.preventDefault();
    dragActiveRef.current = true;
    setDragHover(true);
  };

  const onDragOver = (e: React.DragEvent<HTMLDivElement>) => {
    if (!dragHasCapture(e.dataTransfer)) return;
    e.preventDefault(); // required — without it the drop never fires
    e.dataTransfer.dropEffect = "copy";
    if (!dragActiveRef.current) {
      dragActiveRef.current = true;
      setDragHover(true);
    }
  };

  const onDragLeave = (e: React.DragEvent<HTMLDivElement>) => {
    // relatedTarget is null only when the pointer leaves the window entirely;
    // moving between children fires dragleave with a non-null target.
    if (e.relatedTarget) return;
    dragActiveRef.current = false;
    setDragHover(false);
  };

  const onDrop = (e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    dragActiveRef.current = false;
    setDragHover(false);
    void runCapture(extractCaptureItems(e.dataTransfer));
  };

  // ── Actions ──────────────────────────────────────────────────────────────
  const sendQuickCapture = useCallback(() => {
    const text = quickText.trim();
    if (!text) return;
    setQuickText("");
    void emitOrbCommand({ kind: "capture", payload: { text, urls: [], filePaths: [] } });
    pushToast("已送入收集箱 ✓", "ok", 2000);
  }, [quickText, pushToast]);

  const onScenarioClick = useCallback(
    (scenario: Scenario) => {
      const text = quickText.trim();
      if (scenario.kind === "capture" && !text) {
        quickInputRef.current?.focus();
        return;
      }
      setQuickText("");
      // Scenario command (not raw capture): the main window needs the id to
      // apply the scenario's own agent/model/target/template prefs.
      void emitOrbCommand({
        kind: "scenario",
        scenarioId: scenario.id,
        payload: { text, urls: [], filePaths: [] },
      });
      if (scenario.kind === "capture") pushToast("已送入收集箱 ✓", "ok", 2000);
    },
    [quickText, pushToast]
  );

  const openSession = useCallback((sessionId: string) => {
    void emitOrbCommand({ kind: "openSession", sessionId });
  }, []);

  const showMain = useCallback(() => {
    collapse();
    void emitOrbCommand({ kind: "showMain" });
  }, [collapse]);

  const newChat = useCallback(() => {
    collapse();
    void emitOrbCommand({ kind: "newChat" });
  }, [collapse]);

  const quit = useCallback(() => {
    if (!isTauriRuntime()) return;
    void invoke("quit_app").catch(() => undefined);
  }, []);

  const lastToast = toasts.length > 0 ? toasts[toasts.length - 1] : null;

  return (
    <div
      className="orb-root"
      onDragEnter={onDragEnter}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {mode === "panel" ? (
        <OrbPanel
          orbState={orbState}
          scenarios={scenarios}
          quickText={quickText}
          onQuickTextChange={setQuickText}
          onSendQuickCapture={sendQuickCapture}
          onScenarioClick={onScenarioClick}
          onCollapse={collapse}
          onOpenSession={openSession}
          onShowMain={showMain}
          onNewChat={newChat}
          onQuit={quit}
          quota={quota}
          quotaBusy={quotaBusy}
          onQuotaRefresh={() => void refreshQuota(true)}
          dragHover={dragHover}
          nowMs={nowMs}
          toasts={toasts}
          processingCount={processingCount}
          quickInputRef={quickInputRef}
        />
      ) : (
        <OrbBall
          worstNotice={worstNoticeKind(orbState.notices.map((n) => n.kind))}
          processingCount={processingCount}
          toastText={lastToast ? lastToast.text : null}
          toastTone={lastToast ? lastToast.tone : "ok"}
          dragHover={dragHover}
          onActivate={expand}
        />
      )}
    </div>
  );
}
