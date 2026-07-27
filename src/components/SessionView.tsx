import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, Eye, EyeOff, FileText, MessageSquareQuote, Pencil, Plus, Square, TerminalSquare, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { extractAcpUpdateText, mergeStreamText, userMessageAnchorId } from "../lib/acpTranscript";
import {
  activityBarLabel,
  activityHealth,
  formatAgo,
  isSubagentTool,
  isToolInProgress,
  stallBannerCopy,
  type ActivityHealth,
} from "../lib/activityHealth";
import { isTauriRuntime, readTerminalSnapshot, resizeTerminal, startTerminal, writeTerminal } from "../lib/api";
import { newQuotePinId, type QuotePin } from "../lib/quoteComment";
import type { AcpEvent, AgentConfig, CapabilitySnapshot, Session, SessionEvent, SessionStatus, SessionViewMode, TerminalOutput } from "../lib/types";
import { ClippedBody } from "./ClippedBody";
import { LinkCwdContext, LinkedText } from "./LinkedText";
import { MarkdownBody } from "./MarkdownBody";
import { MessageOutline } from "./MessageOutline";
import { MessageTimestamp } from "./MessageTimestamp";


export type UserMessageAnchor = {
  messageId?: string;
  createdAt: string;
  text: string;
};

type SessionViewProps = {
  agent: AgentConfig;
  events: SessionEvent[];
  session: Session;
  viewMode: SessionViewMode;
  openSessions: Session[];
  /** Sticky auth / account warning for the active agent (e.g. Claude not logged in). */
  authBanner?: string | null;
  /** Start native login (browser/CLI). */
  onSignIn?: () => void | Promise<void>;
  signInBusy?: boolean;
  onTabSelect: (session: Session) => void;
  onTabClose: (sessionId: string) => void;
  onNewTab: () => void;
  onSessionStatusChange?: (status: SessionStatus) => void;
  onCapabilities?: (caps: import("../lib/types").CapabilitySnapshot | null) => void;
  onViewModeToggle: () => void;
  /** P2-UX-4: edit You → truncate following + resend. */
  onEditResend?: (anchor: UserMessageAnchor, newText: string) => void | Promise<void>;
  /** Last ACP/PTY activity ms for this session (heartbeat / stale). */
  lastActivityAt?: number | null;
  /** Pending inline quote-comments for this dialog (sent with Composer text). */
  quotePins?: QuotePin[];
  onQuotePinsChange?: (pins: QuotePin[]) => void;
  /** Interrupt the live turn (same as Esc×2 / ■). */
  onInterrupt?: () => void | Promise<void>;
};

/** Tabs-only for the shared workspace-titlebar (rendered by App.tsx). */
export function SessionTabs({
  openSessions,
  session,
  onTabSelect,
  onTabClose,
  onNewTab,
  onRenameSession,
}: {
  openSessions: Session[];
  session: Session;
  onTabSelect: (session: Session) => void;
  onTabClose: (sessionId: string) => void;
  onNewTab: () => void;
  onRenameSession?: (sessionId: string, label: string) => void;
}) {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!renamingId) return;
    renameInputRef.current?.focus();
    renameInputRef.current?.select();
  }, [renamingId]);

  const commitRename = () => {
    if (!renamingId || !onRenameSession) {
      setRenamingId(null);
      return;
    }
    onRenameSession(renamingId, renameDraft.trim() || "New session");
    setRenamingId(null);
  };

  return (
    <div className="editor-tabs__tablist" role="tablist" aria-label="Session tabs">
      {openSessions.map((openSession) => {
        const busy =
          openSession.status === "running" || openSession.status === "starting";
        const isRenaming = renamingId === openSession.id;
        return (
          <div
            className={
              openSession.id === session.id
                ? `editor-tab is-active is-${openSession.status}`
                : `editor-tab is-${openSession.status}`
            }
            key={openSession.id}
            role="tab"
            aria-selected={openSession.id === session.id}
          >
            {busy && (
              <span
                className={`editor-tab__pulse is-${openSession.status}`}
                aria-hidden
              />
            )}
            {isRenaming ? (
              <input
                ref={renameInputRef}
                className="editor-tab__rename"
                value={renameDraft}
                aria-label="Rename tab"
                onChange={(e) => setRenameDraft(e.target.value)}
                onBlur={() => commitRename()}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    commitRename();
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    setRenamingId(null);
                  }
                }}
                onClick={(e) => e.stopPropagation()}
              />
            ) : (
              <button
                className="editor-tab__select"
                type="button"
                title={`${openSession.cwd} · double-click to rename`}
                onClick={() => onTabSelect(openSession)}
                onDoubleClick={(e) => {
                  if (!onRenameSession) return;
                  e.preventDefault();
                  e.stopPropagation();
                  setRenamingId(openSession.id);
                  setRenameDraft(openSession.label);
                }}
              >
                <span>{openSession.label}</span>
              </button>
            )}
            <button className="editor-tab__close" type="button" title={`Close ${openSession.label}`} aria-label={`Close ${openSession.label}`} onClick={() => onTabClose(openSession.id)}>
              <X size={12} />
            </button>
          </div>
        );
      })}
      <button className="editor-tab__new" type="button" title="New conversation" aria-label="New conversation" onClick={onNewTab}>
        <Plus size={14} />
      </button>
    </div>
  );
}

export function SessionView({
  agent,
  events,
  session,
  viewMode,
  openSessions,
  authBanner = null,
  onSignIn,
  signInBusy = false,
  onTabSelect,
  onTabClose,
  onNewTab,
  onSessionStatusChange,
  onCapabilities,
  onViewModeToggle,
  onEditResend,
  lastActivityAt = null,
  quotePins = [],
  onQuotePinsChange,
  onInterrupt,
}: SessionViewProps) {
  // Show thinking/tool rows by default (they render collapsed). Eye can hide them entirely.
  const [detailsVisible, setDetailsVisible] = useState(true);
  const showRaw = viewMode === "raw-terminal";
  // Only a real turn earns the top bar. Connecting is background work — the
  // composer floats a pill for it so the stage never resizes mid-reconnect.
  const isLive = session.status === "running";

  return (
    <section className="session-view" aria-label="Session view">

      {isLive && (
        <SessionActivityBar status={session.status} lastActivityAt={lastActivityAt} />
      )}

      {authBanner && (
        <div className="session-auth-banner" role="status">
          <div className="session-auth-banner__copy">
            <strong>Sign in required</strong>
            <span>{authBanner}</span>
          </div>
          {onSignIn && (
            <button
              className="session-auth-banner__button"
              type="button"
              disabled={signInBusy}
              onClick={() => void onSignIn()}
            >
              {signInBusy ? "Opening login…" : "Sign in with Claude"}
            </button>
          )}
        </div>
      )}

      {/*
        First principles: transport is always running. Clean/Raw are only presentation.
        Hiding (not unmounting) keeps PTY/ACP alive while Clean is primary.
      */}
      <div className={showRaw ? "session-stage is-raw" : "session-stage is-clean"}>
        <div className="session-stage__clean" hidden={showRaw} aria-hidden={showRaw}>
          <CleanPlaceholder
            agent={agent}
            session={session}
            events={events}
            detailsVisible={detailsVisible}
            onDetailsToggle={() => setDetailsVisible((visible) => !visible)}
            onShowRaw={onViewModeToggle}
            authBanner={authBanner}
            onSignIn={onSignIn}
            signInBusy={signInBusy}
            onEditResend={onEditResend}
            lastActivityAt={lastActivityAt}
            quotePins={quotePins}
            onQuotePinsChange={onQuotePinsChange}
            onInterrupt={onInterrupt}
          />
        </div>
        <div className="session-stage__raw" data-active={showRaw ? "true" : "false"} aria-hidden={!showRaw}>
          <RawTerminal
            agent={agent}
            session={session}
            visible={showRaw}
            onSessionStatusChange={onSessionStatusChange}
            onCapabilities={onCapabilities}
          />
        </div>
      </div>
    </section>
  );
}

function RawTerminal({
  agent,
  session,
  visible,
  onSessionStatusChange,
  onCapabilities,
}: {
  agent: AgentConfig;
  session: Session;
  visible: boolean;
  onSessionStatusChange?: (status: SessionStatus) => void;
  onCapabilities?: (caps: CapabilitySnapshot | null) => void;
}) {
  const reportStatus = useCallback((nextStatus: SessionStatus) => {
    onSessionStatusChange?.(nextStatus);
  }, [onSessionStatusChange]);

  return (
    <div className="terminal-frame">
      <div className="terminal-toolbar">
        <span>
          <TerminalSquare size={15} />
          {agent.label}
          {!visible && <em className="terminal-toolbar__bg"> · running in background</em>}
        </span>
      </div>
      {agent.transport === "acp" ? (
        <AcpTerminalSurface
          sessionId={session.id}
          cwd={session.cwd}
          command={agent.command}
          args={agent.args}
          onStatusChange={reportStatus}
          onCapabilities={onCapabilities}
        />
      ) : (
        <TerminalSurface
          sessionId={session.id}
          cwd={session.cwd}
          command={agent.command}
          args={agent.args}
          agentLabel={agent.label}
          visible={visible}
          onStatusChange={reportStatus}
        />
      )}
    </div>
  );
}

function AcpTerminalSurface({
  sessionId,
  cwd,
  command,
  args,
  onStatusChange,
  onCapabilities,
}: {
  sessionId: string;
  cwd: string;
  command: string;
  args: string[];
  onStatusChange: (status: SessionStatus) => void;
  onCapabilities?: (caps: CapabilitySnapshot | null) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  // Keep signature stable for call sites; lifecycle is owned by App (lazy warm).
  void cwd;
  void command;
  void args;
  void onCapabilities;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const style = getComputedStyle(document.documentElement);
    const terminal = new Terminal({
      convertEol: true,
      cursorBlink: false,
      fontFamily: '"Cascadia Mono", Consolas, monospace',
      fontSize: 13,
      theme: {
        background: style.getPropertyValue("--terminal-bg").trim(),
        foreground: style.getPropertyValue("--text").trim(),
        cursor: style.getPropertyValue("--accent").trim()
      }
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(host);
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // Per-session stream buffer so we only write *new* characters to xterm
    let assistantBuffer = "";
    const fit = () => fitAddon.fit();
    const resizeObserver = new ResizeObserver(fit);
    resizeObserver.observe(host);

    // Passive only — App owns lazy ACP start (warm on type / ensure on send).
    // Auto-starting here freezes the shell on every tab open.
    terminal.write(
      "ACP idle · type in the composer to warm the agent in the background.\r\n"
    );

    const setup = async () => {
      if (!isTauriRuntime()) {
        terminal.write("ACP requires the Tauri desktop runtime.\r\n");
        return;
      }
      try {
        const dispose = await listen<AcpEvent>("acp-event", (event) => {
          const payload = event.payload;
          if (payload.sessionId !== sessionId || disposed) return;

          if (payload.method === "session/starting") {
            const data = payload.data as { hint?: string; command?: string; args?: string[] } | null;
            const line = data?.command
              ? [data.command, ...(data.args ?? [])].join(" ")
              : data?.hint ?? "starting…";
            terminal.write(`[starting] ${line}\r\n`);
            return;
          }

          if (payload.method === "session/update") {
            const extracted = extractAcpUpdateText(payload.data);
            if (extracted?.role === "assistant" && extracted.text) {
              const next = mergeStreamText(assistantBuffer, extracted.text, extracted.isDelta);
              if (next.length > assistantBuffer.length && next.startsWith(assistantBuffer)) {
                terminal.write(next.slice(assistantBuffer.length));
                assistantBuffer = next;
              } else if (next !== assistantBuffer && assistantBuffer.length === 0) {
                terminal.write(next);
                assistantBuffer = next;
              } else if (next.startsWith(assistantBuffer) && next.length > assistantBuffer.length) {
                terminal.write(next.slice(assistantBuffer.length));
                assistantBuffer = next;
              }
              return;
            }
            if (extracted?.role === "thought" && extracted.text) {
              terminal.write(extracted.isDelta ? extracted.text : `\r\n[think] ${extracted.text}`);
              return;
            }
            if (extracted?.role === "tool") {
              const title = extracted.toolTitle ?? "tool";
              const status = extracted.toolStatus ?? "";
              terminal.write(`\r\n[tool] ${title}${status ? ` (${status})` : ""}\r\n`);
              return;
            }
          }

          if (payload.kind === "system" && payload.method === "session/ready") {
            terminal.write("[ready] ACP session ready\r\n");
            onStatusChange("waiting");
            return;
          }
          if (payload.method === "rpc/response") {
            terminal.write("\r\n[turn complete]\r\n");
            assistantBuffer = "";
            if (payload.kind === "error") onStatusChange("error");
            else onStatusChange("waiting");
            return;
          }
        });
        if (disposed) {
          dispose();
          return;
        }
        unlisten = dispose;
        requestAnimationFrame(fit);
      } catch (error) {
        terminal.write(`ACP listener error: ${String(error)}\r\n`);
      }
    };
    void setup();
    requestAnimationFrame(fit);
    return () => {
      disposed = true;
      unlisten?.();
      resizeObserver.disconnect();
      terminal.dispose();
      // Do NOT stopAcpSession here: Clean↔Raw toggle must keep the agent alive.
    };
  }, [onStatusChange, sessionId]);

  return <div ref={hostRef} className="terminal-surface" aria-label="ACP session output" />;
}

function TerminalSurface({
  sessionId,
  cwd,
  command,
  args,
  agentLabel,
  visible,
  onStatusChange
}: {
  sessionId: string;
  cwd: string;
  command: string;
  args: string[];
  agentLabel: string;
  visible: boolean;
  onStatusChange: (status: SessionStatus) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const argsKey = args.join("\0");
  const visibleRef = useRef(visible);
  visibleRef.current = visible;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const style = getComputedStyle(document.documentElement);
    // Full-screen TUI agents need real VT handling. convertEol:true breaks them.
    const isWindows = navigator.userAgent.includes("Windows");
    const terminal = new Terminal({
      convertEol: false,
      cursorBlink: true,
      fontFamily: '"Cascadia Mono", Consolas, "Courier New", monospace',
      fontSize: 13,
      lineHeight: 1.2,
      scrollback: 5000,
      allowTransparency: false,
      cols: 120,
      rows: 40,
      ...(isWindows
        ? {
            windowsPty: {
              backend: "conpty" as const,
              buildNumber: 22621,
            },
          }
        : {}),
      theme: {
        background: style.getPropertyValue("--terminal-bg").trim() || "#0f1115",
        foreground: style.getPropertyValue("--text").trim() || "#e6e6e6",
        cursor: style.getPropertyValue("--accent").trim() || "#6cb6ff",
      },
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(host);

    let disposed = false;
    let unlisten: (() => void) | undefined;
    const fitAndResize = () => {
      // When Clean is primary, host may be 1×1 — keep a usable PTY geometry for the agent.
      if (visibleRef.current) {
        try {
          fitAddon.fit();
        } catch {
          /* ignore */
        }
      }
      const cols = visibleRef.current && terminal.cols >= 40 ? terminal.cols : Math.max(terminal.cols, 120);
      const rows = visibleRef.current && terminal.rows >= 12 ? terminal.rows : Math.max(terminal.rows, 40);
      if (isTauriRuntime()) {
        void resizeTerminal(sessionId, cols, rows).catch(() => undefined);
      }
    };
    const resizeObserver = new ResizeObserver(() => {
      requestAnimationFrame(fitAndResize);
    });
    resizeObserver.observe(host);
    const input = terminal.onData((data) => {
      void writeTerminal(sessionId, data).catch(() => undefined);
    });

    const setup = async () => {
      if (sessionId.startsWith("session-empty-")) {
        terminal.write("Create a session from the project list to start a terminal.\r\n");
        onStatusChange("exited");
        return;
      }
      if (!isTauriRuntime()) {
        terminal.write("AgentShell Raw Terminal requires the Tauri desktop runtime.\r\n");
        onStatusChange("error");
        return;
      }

      try {
        const dispose = await listen<TerminalOutput>("session-output", (event) => {
          const output = event.payload;
          if (output.sessionId !== sessionId || disposed) return;
          if (output.data) terminal.write(output.data);
          if (output.error) {
            terminal.write(`\r\n[terminal error] ${output.error}\r\n`);
            onStatusChange("error");
          }
          if (output.exited) onStatusChange("exited");
        });
        if (disposed) {
          dispose();
          return;
        }
        unlisten = dispose;

        fitAndResize();

        const snapshot = await readTerminalSnapshot(sessionId, cwd);
        const reattaching = Boolean(snapshot && snapshot.length > 0);

        onStatusChange("starting");
        await startTerminal(sessionId, cwd, command, args);
        if (disposed) return;

        if (reattaching) {
          terminal.reset();
          terminal.write(snapshot);
        }

        onStatusChange("running");
        requestAnimationFrame(() => {
          fitAndResize();
          requestAnimationFrame(fitAndResize);
        });
      } catch (error) {
        terminal.write(
          `\r\nAgentShell could not start ${agentLabel}: ${String(error)}\r\n` +
            `Command: ${[command, ...args].filter(Boolean).join(" ")}\r\n`,
        );
        onStatusChange("error");
      }
    };
    void setup();

    return () => {
      disposed = true;
      unlisten?.();
      input.dispose();
      resizeObserver.disconnect();
      terminal.dispose();
    };
  }, [agentLabel, argsKey, command, cwd, onStatusChange, sessionId]);

  // When becoming visible, re-fit to the real panel size
  useEffect(() => {
    if (!visible) return;
    const id = requestAnimationFrame(() => {
      const host = hostRef.current;
      if (!host) return;
      // Trigger resize observer path by dispatching a fake size check via window event
      window.dispatchEvent(new Event("resize"));
    });
    return () => cancelAnimationFrame(id);
  }, [visible]);

  return <div ref={hostRef} className="terminal-surface" aria-label="Raw terminal" />;
}

function CleanPlaceholder({
  agent,
  session,
  events,
  detailsVisible,
  onDetailsToggle,
  onShowRaw,
  authBanner = null,
  onSignIn,
  signInBusy = false,
  onEditResend,
  lastActivityAt = null,
  quotePins = [],
  onQuotePinsChange,
  onInterrupt,
}: {
  agent: AgentConfig;
  session: Session;
  events: SessionEvent[];
  detailsVisible: boolean;
  onDetailsToggle: () => void;
  onShowRaw?: () => void;
  authBanner?: string | null;
  onSignIn?: () => void | Promise<void>;
  signInBusy?: boolean;
  onEditResend?: (anchor: UserMessageAnchor, newText: string) => void | Promise<void>;
  lastActivityAt?: number | null;
  quotePins?: QuotePin[];
  onQuotePinsChange?: (pins: QuotePin[]) => void;
  onInterrupt?: () => void | Promise<void>;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  /** When true, new content may stick the viewport to the bottom. */
  const stickToBottomRef = useRef(true);
  /** State mirror of stickToBottom so the jump-to-bottom chip can re-render. */
  const [atBottom, setAtBottom] = useState(true);
  const isPty = agent.transport === "pty";
  const isRunning = session.status === "running";
  // Ignore raw_chunk dumps in Clean (legacy / accidental) — they are not You/Thinking/Reply.
  const visibleEvents = useMemo(
    () => events.filter((e) => e.type !== "raw_chunk"),
    [events]
  );
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState("");
  const [editBusy, setEditBusy] = useState(false);
  // Local clock for stall UI only — does not drive list scroll keys.
  const [healthNow, setHealthNow] = useState(() => Date.now());
  useEffect(() => {
    if (!isRunning) return;
    const id = window.setInterval(() => setHealthNow(Date.now()), 2000);
    return () => window.clearInterval(id);
  }, [isRunning]);

  // After send: last card is still You → show ghost "Waiting for agent…"
  const lastVisible = visibleEvents[visibleEvents.length - 1];
  const awaitingFirstChunk =
    isRunning &&
    (!lastVisible ||
      lastVisible.type === "user_message" ||
      lastVisible.type === "handoff_prepared");

  const health = activityHealth(session.status, lastActivityAt, healthNow);
  const openTool = useMemo(() => {
    for (let i = visibleEvents.length - 1; i >= 0; i -= 1) {
      const e = visibleEvents[i];
      if (e.type === "tool_call" && isToolInProgress(e.status)) {
        return e;
      }
    }
    return null;
  }, [visibleEvents]);
  const midTurn = isRunning && !awaitingFirstChunk;
  // Stall copy is about a live turn going quiet. A reconnect is not a stalled
  // turn — inserting this card while warming up only shoves the transcript.
  const showStallBanner =
    isRunning &&
    (awaitingFirstChunk || health === "quiet" || health === "stalled" || health === "stuck");

  // Content fingerprint — only real transcript changes should pin-scroll (never a clock tick).
  const scrollKey = useMemo(() => {
    const last = visibleEvents[visibleEvents.length - 1];
    const lastSig = last
      ? `${last.type}:${last.createdAt}:${"text" in last ? String(last.text).length : ""}:${"status" in last ? last.status : ""}`
      : "empty";
    return `${session.id}|${visibleEvents.length}|${lastSig}|wait:${awaitingFirstChunk}|health:${health}`;
  }, [session.id, visibleEvents, awaitingFirstChunk, health]);

  const prevAwaitingRef = useRef(false);

  // Own send → jump back to live tail (even if you were reading history).
  useEffect(() => {
    if (awaitingFirstChunk && !prevAwaitingRef.current) {
      stickToBottomRef.current = true;
      setAtBottom(true);
    }
    prevAwaitingRef.current = awaitingFirstChunk;
  }, [awaitingFirstChunk]);

  // Switching dialog → follow the new session's tail by default.
  useEffect(() => {
    stickToBottomRef.current = true;
    setAtBottom(true);
  }, [session.id]);

  const onListScroll = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    // User scrolled up to read history → stop auto-jumping to bottom.
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
    const nearBottom = distance < 80;
    stickToBottomRef.current = nearBottom;
    setAtBottom(nearBottom);
  }, []);

  const jumpToBottom = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    stickToBottomRef.current = true;
    setAtBottom(true);
    el.scrollTop = el.scrollHeight;
  }, []);

  // Stick to bottom only while the user is already near the end (chat apps pattern).
  // Never key this off the 1s activity clock / stale flag — that caused the 5–10s jump bug.
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    if (!stickToBottomRef.current) return;
    el.scrollTop = el.scrollHeight;
  }, [scrollKey]);

  return (
    // Relative paths in agent text resolve against this dialog's project root.
    <LinkCwdContext.Provider value={session.cwd || null}>
    <div className="clean-surface">
      <button
        className="icon-button icon-button--eye"
        type="button"
        title={
          (isPty
            ? "Clean View · You / Thinking / Tool / Reply (from terminal stream)"
            : "Clean View · live stream · thinking/tools collapsed by default")
          + " — " + (detailsVisible ? "hide" : "show") + " details"
        }
        onClick={onDetailsToggle}
      >
        {detailsVisible ? <Eye size={14} /> : <EyeOff size={14} />}
      </button>
      <div className="clean-surface__body">
      {!atBottom && visibleEvents.length > 0 && (
        <button
          type="button"
          className="scroll-to-bottom"
          title="Jump to latest message"
          aria-label="Jump to latest message"
          onClick={jumpToBottom}
        >
          <ChevronDown size={16} />
          <span>Latest</span>
        </button>
      )}
      <div
        ref={listRef}
        className={detailsVisible ? "event-list" : "event-list is-details-hidden"}
        onScroll={onListScroll}
      >
        {/* Quote UI is isolated so its setState does not re-render cards (keeps selection). */}
        {onQuotePinsChange && (
          <QuoteOverlay
            listRef={listRef}
            pins={quotePins}
            onPinsChange={onQuotePinsChange}
          />
        )}
        {/* No connect card here on purpose: warming shows as a floating pill on
            the composer, so the transcript never reflows while reconnecting. */}
        {visibleEvents.length === 0 && (
          <div className="clean-empty" role="status">
            <p className="clean-empty__title">
              {authBanner ? "Sign in to continue" : `Message ${agent.label}`}
            </p>
            <p className="clean-empty__hint">
              {authBanner
                ? "Use the Sign in button above — it opens Claude’s browser login. Come back here when done."
                : isPty
                  ? "Send from the composer. Thinking / tools / replies show up as cards here."
                  : "Type below to warm the agent in the background, then send when ready."}
              {isPty && onShowRaw ? viewModeToggleHint(onShowRaw) : null}
            </p>
            {authBanner && onSignIn && (
              <button
                className="session-auth-banner__button"
                type="button"
                disabled={signInBusy}
                onClick={() => void onSignIn()}
              >
                {signInBusy ? "Opening login…" : "Sign in with Claude"}
              </button>
            )}
          </div>
        )}
        {visibleEvents.map((event, index) => {
          const isCollapsible = event.type === "thought" || event.type === "tool_call";
          const toolRunning =
            event.type === "tool_call" && isToolInProgress(event.status);
          const toolStalled =
            toolRunning && (health === "stalled" || health === "stuck" || health === "quiet");

          const toolStatusLabel = toolStalled
            ? health === "stuck"
              ? "no updates · appears stuck"
              : health === "stalled"
                ? "no updates · may be stuck"
                : "no updates recently"
            : event.type === "tool_call"
              ? event.status
              : undefined;

          // Collapsed summary: fixed short type + SHORT one-line teaser.
          // Full body only lives in the expanded clip box (never in the summary).
          const label =
            event.type === "thought"
              ? "Thinking"
              : event.type === "tool_call"
                ? "Tool"
                : event.type === "user_message"
                  ? "You"
                  : event.type === "assistant_message"
                    ? (event.agentId ? "Reply" : "Notification")
                    : event.type === "handoff_prepared"
                      ? "Handoff"
                      : event.type.replace(/_/g, " ");

          // Handoff: a marker, not a wall of text. The notes themselves ride
          // along with the next message the user sends (never shown here or
          // pasted into the composer).
          const body =
            event.type === "handoff_prepared"
              ? `Notes prepared for **${event.targetAgentId}** — they will be attached to your next message.\n\nFull notes: \`${event.handoffPath}\``
              : event.type === "file_change"
                ? `${event.changeType}: ${event.path}`
                : event.text;

          const oneLine = (s: string) => s.replace(/\s+/g, " ").trim();
          const clipTeaser = (s: string, max = 42) => {
            const t = oneLine(s);
            if (!t) return "";
            return t.length > max ? `${t.slice(0, max - 1)}…` : t;
          };

          // Tools: title + status + which file (never the whole payload) — enough
          // to tell a long tool from a dead one without expanding the card.
          let previewFull = "";
          if (event.type === "tool_call") {
            const fileName = event.path
              ? event.path.split(/[\\/]/).filter(Boolean).pop() ?? ""
              : "";
            const bits = [
              event.title ? oneLine(String(event.title)) : "",
              fileName && fileName !== event.title ? fileName : "",
              toolStatusLabel || (event.status ? String(event.status) : ""),
            ].filter(Boolean);
            previewFull = bits.join(" · ");
          } else if (typeof body === "string") {
            previewFull = oneLine(body);
          }
          const preview = clipTeaser(previewFull, 48) || "…";

          const useMarkdown =
            event.type === "assistant_message" ||
            event.type === "user_message" ||
            event.type === "thought" ||
            event.type === "handoff_prepared";
          // raw_chunk is ANSI-stripped TUI text — keep plain, not markdown

          // Thinking / tool_call: collapsed by default; auto-expand when a tool looks stuck.
          // User message / assistant: always fully visible.
          if (isCollapsible) {
            return (
              <details
                className={`event-card event-card--collapsible event-card--${event.type}${toolRunning ? " is-tool-running" : ""}${toolStalled ? " is-tool-stalled" : ""}${toolStalled && health === "stuck" ? " is-tool-stuck" : ""}`}
                key={`${event.type}-${event.createdAt}-${index}`}
                open={toolStalled && (health === "stalled" || health === "stuck") ? true : undefined}
              >
                <summary className="event-card__summary" title={previewFull || label}>
                  <span className="event-card__type">
                    {label}
                    {toolRunning && !toolStalled && <span className="event-card__live-dot" aria-hidden />}
                    {toolStalled && <span className="event-card__stall-dot" aria-hidden />}
                  </span>
                  <span className="event-card__preview-wrap">
                    <span className="event-card__preview">{preview}</span>
                  </span>
                </summary>
                <div className="event-card__expand-slot">
                  <ClippedBody
                    className="event-card__clip"
                    maxHeight={event.type === "tool_call" ? 220 : 260}
                  >
                    {useMarkdown && typeof body === "string" ? (
                      <MarkdownBody text={body} className="event-card__body event-card__body--clipped-md" />
                    ) : (
                      <pre className="event-card__body event-card__body--tool">
                        {typeof body === "string" ? <LinkedText text={body} /> : body}
                      </pre>
                    )}
                  </ClippedBody>
                </div>
              </details>
            );
          }

          const isUser = event.type === "user_message";
          const userKey = isUser
            ? userMessageAnchorId(event)
            : `${event.type}-${event.createdAt}-${index}`;
          const isEditing = isUser && editingKey === userKey;

          // Mode tone for color accent (You card bar + Reply mode tag)
          const tone =
            isUser && "modeLabel" in event
              ? modeTone(event.modeLabel)
              : !isUser && "modeLabel" in event
                ? modeTone(event.modeLabel)
                : undefined;

          return (
            <article
              className={`event-card event-card--${event.type}${isEditing ? " is-editing" : ""}`}
              key={`${event.type}-${event.createdAt}-${index}`}
              id={isUser ? userKey : undefined}
              {...(tone ? { "data-mode-tone": tone } : {})}
            >
              {event.type !== "user_message" && event.type !== "assistant_message" && (
                <div className="event-card__icon">
                  <FileText size={15} />
                </div>
              )}
              <div className="event-card__main">
                <div className="event-card__type-row">
                  <span className="event-card__type">{label}</span>
                  <span className="event-card__type-row__right">
                    <span className="event-card__time-edit">
                      <MessageTimestamp createdAt={event.createdAt} />
                      {isUser && onEditResend && !isEditing && (
                        <button
                          type="button"
                          className="event-card__edit"
                          title="Edit & resend (drops later messages)"
                          aria-label="Edit and resend this message"
                          onClick={() => {
                            setEditingKey(userKey);
                            setEditDraft(event.text);
                          }}
                        >
                          <Pencil size={12} />
                          Edit
                        </button>
                      )}
                    </span>
                  </span>
                </div>
                {isEditing ? (
                  <div className="event-card__edit-form">
                    <textarea
                      className="event-card__edit-input"
                      value={editDraft}
                      rows={Math.min(12, Math.max(3, editDraft.split("\n").length + 1))}
                      disabled={editBusy}
                      autoFocus
                      onChange={(e) => setEditDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Escape") {
                          e.preventDefault();
                          setEditingKey(null);
                        }
                        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                          e.preventDefault();
                          const next = editDraft.trim();
                          if (!next || editBusy) return;
                          setEditBusy(true);
                          void Promise.resolve(
                            onEditResend?.(
                              {
                                messageId: event.messageId,
                                createdAt: event.createdAt,
                                text: event.text,
                              },
                              next
                            )
                          ).finally(() => {
                            setEditBusy(false);
                            setEditingKey(null);
                          });
                        }
                      }}
                    />
                    <p className="event-card__edit-hint">
                      Resend truncates everything after this message. Ctrl+Enter to confirm.
                    </p>
                    <div className="event-card__edit-actions">
                      <button
                        type="button"
                        className="event-card__edit-cancel"
                        disabled={editBusy}
                        onClick={() => setEditingKey(null)}
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        className="event-card__edit-confirm"
                        disabled={editBusy || !editDraft.trim()}
                        onClick={() => {
                          const next = editDraft.trim();
                          if (!next) return;
                          setEditBusy(true);
                          void Promise.resolve(
                            onEditResend?.(
                              {
                                messageId: event.messageId,
                                createdAt: event.createdAt,
                                text: event.text,
                              },
                              next
                            )
                          ).finally(() => {
                            setEditBusy(false);
                            setEditingKey(null);
                          });
                        }}
                      >
                        {editBusy ? "Resending…" : "Resend from here"}
                      </button>
                    </div>
                  </div>
                ) : useMarkdown && typeof body === "string" ? (
                  <MarkdownBody text={body} />
                ) : (
                  <pre className="event-card__plain">
                    {typeof body === "string" ? <LinkedText text={body} /> : body}
                  </pre>
                )}
                {/* Reply metadata: mode · agent · model · effort · duration (only for real replies with metadata) */}
                {event.type === "assistant_message" && event.agentId && (
                  <div className="event-card__meta">
                    {event.modeLabel && (
                      <>
                        <span
                          className="meta-tag__symbol"
                          data-mode-tone={modeTone(event.modeLabel)}
                        >
                          ▣
                        </span>
                        <span
                          className="meta-tag meta-tag--mode"
                          data-mode-tone={modeTone(event.modeLabel)}
                        >
                          {event.modeLabel}
                        </span>
                      </>
                    )}
                    {event.agentLabel && (
                      <span className="meta-tag">{event.agentLabel}</span>
                    )}
                    {event.modelLabel && (
                      <span className="meta-tag">{event.modelLabel}</span>
                    )}
                    {event.effortLabel && (
                      <span className="meta-tag">{event.effortLabel}</span>
                    )}
                    {event.durationMs != null && (
                      <span className="meta-tag meta-tag--duration">
                        {(event.durationMs / 1000).toFixed(1)}s
                      </span>
                    )}
                  </div>
                )}
              </div>
            </article>
          );
        })}
        {showStallBanner && (
          <StallBanner
            health={health}
            midTurn={midTurn}
            openToolTitle={openTool?.title ?? null}
            openToolIsSubagent={isSubagentTool(openTool?.toolName)}
            lastActivityAt={lastActivityAt}
            now={healthNow}
            onInterrupt={onInterrupt}
          />
        )}
      </div>
      <MessageOutline events={visibleEvents} sessionId={session.id} />
      </div>
    </div>
    </LinkCwdContext.Provider>
  );
}

/**
 * Floating quote / comment UI. Own state only — never re-renders event cards,
 * so browser text selection highlight stays after mouseup.
 */
function QuoteOverlay({
  listRef,
  pins,
  onPinsChange,
}: {
  listRef: React.RefObject<HTMLDivElement | null>;
  pins: QuotePin[];
  onPinsChange: (pins: QuotePin[]) => void;
}) {
  const [btn, setBtn] = useState<{ quoted: string; x: number; y: number } | null>(null);
  const [draft, setDraft] = useState<{
    quoted: string;
    x: number;
    y: number;
    comment: string;
  } | null>(null);
  const draftRootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const pinsRef = useRef(pins);
  pinsRef.current = pins;
  const committingRef = useRef(false);

  const readSelection = useCallback(() => {
    const list = listRef.current;
    if (!list) return null;
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !sel.rangeCount) return null;
    const text = sel.toString().replace(/\r\n/g, "\n").trim();
    if (text.length < 1) return null;
    const range = sel.getRangeAt(0);
    if (!list.contains(range.commonAncestorContainer)) return null;
    const rect = range.getBoundingClientRect();
    // Ignore zero-size (can happen after reflow)
    if (rect.width < 1 && rect.height < 1) return null;
    const host = list.getBoundingClientRect();
    return {
      quoted: text,
      x: Math.min(Math.max(16, rect.left - host.left + rect.width / 2), Math.max(40, host.width - 16)),
      y: Math.max(12, rect.top - host.top + list.scrollTop),
    };
  }, [listRef]);

  // Attach to the list element so we don't rely on React bubble (and avoid parent setState).
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const onUp = (e: MouseEvent) => {
      if (draftRef.current) return;
      // Ignore mouseup that originated on our own chrome
      if (e.target instanceof Node) {
        const t = e.target as HTMLElement;
        if (t.closest(".quote-pop, .quote-draft, .quote-pin")) return;
      }
      const pos = readSelection();
      setBtn(pos);
    };
    list.addEventListener("mouseup", onUp);
    return () => list.removeEventListener("mouseup", onUp);
  }, [listRef, readSelection]);

  const commit = useCallback(
    (d: { quoted: string; x: number; y: number; comment: string }) => {
      if (committingRef.current) return;
      const comment = d.comment.trim();
      if (!comment) {
        setDraft(null);
        return;
      }
      committingRef.current = true;
      const pin: QuotePin = {
        id: newQuotePinId(),
        quoted: d.quoted,
        comment,
        x: d.x,
        y: d.y + 36,
      };
      onPinsChange([...pinsRef.current, pin]);
      setDraft(null);
      setBtn(null);
      window.setTimeout(() => {
        committingRef.current = false;
      }, 250);
    },
    [onPinsChange]
  );

  // Focus input when draft opens — do NOT use onBlur-to-cancel (that ate the empty box).
  useEffect(() => {
    if (!draft) return;
    const t = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => window.clearTimeout(t);
  }, [draft]);

  // Click outside draft → pin if has text, else close (mousedown so we don't fight focus).
  useEffect(() => {
    if (!draft) return;
    const onDocDown = (e: MouseEvent) => {
      const root = draftRootRef.current;
      if (root && e.target instanceof Node && root.contains(e.target)) return;
      const d = draftRef.current;
      if (!d) return;
      if (d.comment.trim()) commit(d);
      else setDraft(null);
    };
    // Delay so the 评论 button click that opened the draft doesn't immediately close it.
    const t = window.setTimeout(() => {
      document.addEventListener("mousedown", onDocDown);
    }, 0);
    return () => {
      window.clearTimeout(t);
      document.removeEventListener("mousedown", onDocDown);
    };
  }, [draft, commit]);

  return (
    <>
      {pins.map((pin, index) => (
        <button
          key={pin.id}
          type="button"
          className="quote-pin"
          style={{ left: pin.x, top: pin.y }}
          title={`${index + 1}. ${pin.comment}`}
          aria-label={`Comment ${index + 1}`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            setDraft({
              quoted: pin.quoted,
              x: pin.x,
              y: Math.max(12, pin.y - 36),
              comment: pin.comment,
            });
            onPinsChange(pins.filter((p) => p.id !== pin.id));
            setBtn(null);
          }}
        >
          {index + 1}
        </button>
      ))}

      {btn && !draft && (
        <div className="quote-pop" style={{ left: btn.x, top: btn.y }} role="toolbar">
          <button
            type="button"
            className="quote-pop__btn"
            onMouseDown={(e) => {
              // Critical: preventDefault keeps the text selection highlight.
              e.preventDefault();
              e.stopPropagation();
            }}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              const pos = readSelection() ?? btn;
              setDraft({
                quoted: pos.quoted,
                x: pos.x,
                y: pos.y,
                comment: "",
              });
              setBtn(null);
            }}
          >
            <MessageSquareQuote size={13} />
            评论
          </button>
        </div>
      )}

      {draft && (
        <div
          ref={draftRootRef}
          className="quote-draft"
          style={{ left: draft.x, top: draft.y }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <div className="quote-draft__quote" title={draft.quoted}>
            {draft.quoted.length > 120 ? `${draft.quoted.slice(0, 120)}…` : draft.quoted}
          </div>
          <textarea
            ref={inputRef}
            className="quote-draft__input"
            rows={3}
            placeholder="写评论…（Ctrl+Enter 落针）"
            value={draft.comment}
            onChange={(e) => setDraft((d) => (d ? { ...d, comment: e.target.value } : d))}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                setDraft(null);
              }
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                e.preventDefault();
                commit(draft);
              }
            }}
          />
          <div className="quote-draft__actions">
            <button
              type="button"
              className="quote-draft__cancel"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setDraft(null)}
            >
              取消
            </button>
            <button
              type="button"
              className="quote-draft__ok"
              onMouseDown={(e) => e.preventDefault()}
              disabled={!draft.comment.trim()}
              onClick={() => commit(draft)}
            >
              落针
            </button>
          </div>
        </div>
      )}
    </>
  );
}

/** Isolated clock — does NOT re-render the event list (fixes selection wipe). */
function SessionActivityBar({
  status,
  lastActivityAt,
}: {
  status: SessionStatus;
  lastActivityAt: number | null;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);
  const health = activityHealth(status, lastActivityAt, now);
  const barLabel = activityBarLabel(status, health);
  if (!barLabel) return null;
  const severity =
    health === "stuck" ? "stuck" : health === "stalled" ? "stalled" : health === "quiet" ? "stale" : "";
  return (
    <div
      className={`session-activity${severity ? ` is-${severity}` : ""} is-${status}`}
      role="status"
      aria-live="polite"
    >
      <span className="session-activity__pulse" aria-hidden />
      <span className="session-activity__label">{barLabel}</span>
      {lastActivityAt != null && (
        <span className="session-activity__ago">
          last update {formatAgo(lastActivityAt, now)}
        </span>
      )}
      {status === "running" && (
        <span className="session-activity__hint">
          {health === "stuck" || health === "stalled"
            ? "Esc×2 or ■ to interrupt"
            : "Esc×2 to interrupt"}
        </span>
      )}
    </div>
  );
}

/** Mid-turn / first-chunk stall card with clear severity + interrupt CTA. */
function StallBanner({
  health,
  midTurn,
  openToolTitle,
  openToolIsSubagent = false,
  lastActivityAt,
  now,
  onInterrupt,
}: {
  health: ActivityHealth;
  midTurn: boolean;
  openToolTitle?: string | null;
  openToolIsSubagent?: boolean;
  lastActivityAt: number | null;
  now: number;
  onInterrupt?: () => void | Promise<void>;
}) {
  const ago = lastActivityAt != null ? formatAgo(lastActivityAt, now) : undefined;
  const copy =
    stallBannerCopy(health, { midTurn, openToolTitle, openToolIsSubagent, ago }) ??
    (midTurn
      ? null
      : {
          title: "Waiting for agent",
          body: "Message sent. Streaming will show thinking / tools / reply here.",
        });
  if (!copy) return null;

  const severity =
    health === "stuck" ? "stuck" : health === "stalled" ? "stalled" : health === "quiet" ? "stale" : "";
  const showInterrupt =
    Boolean(onInterrupt) && (health === "quiet" || health === "stalled" || health === "stuck");

  return (
    <article
      className={`event-card event-card--waiting${severity ? ` is-${severity}` : ""}`}
      role="status"
      aria-live="polite"
    >
      <div className="event-card__icon">
        {health === "live" || health === "idle" ? (
          <span className="activity-dots" aria-hidden>
            <i /><i /><i />
          </span>
        ) : (
          <span className={`event-card__stall-mark is-${severity || "stale"}`} aria-hidden>
            !
          </span>
        )}
      </div>
      <div className="event-card__main">
        <span className="event-card__type">{copy.title}</span>
        <p className="event-card__waiting-copy">{copy.body}</p>
        {showInterrupt && (
          <div className="event-card__stall-actions">
            <button
              type="button"
              className="event-card__interrupt"
              onClick={() => void onInterrupt?.()}
            >
              <Square size={11} fill="currentColor" />
              Interrupt turn
            </button>
            <span className="event-card__stall-hint">or press Esc twice</span>
          </div>
        )}
      </div>
    </article>
  );
}

/** Map a mode id to a tone category for colored accent. */
function modeTone(modeId: string | null | undefined): string {
  const id = (modeId ?? "").toLowerCase();
  if (!id) return "default";
  if (id.includes("plan")) return "plan";
  if (id.includes("ask") || id.includes("chat") || id.includes("talk")) return "ask";
  if (id.includes("debug") || id.includes("review")) return "debug";
  if (
    id.includes("build") ||
    id.includes("agent") ||
    id.includes("code") ||
    id.includes("edit") ||
    id.includes("default") ||
    id.includes("auto")
  ) {
    return "build";
  }
  return "default";
}

function viewModeToggleHint(onShowRaw: () => void) {
  return (
    <>
      {" "}
      <button type="button" className="link-button" onClick={onShowRaw}>
        Switch to Raw Terminal
      </button>
    </>
  );
}
