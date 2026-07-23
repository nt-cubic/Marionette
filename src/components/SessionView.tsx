import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { listen } from "@tauri-apps/api/event";
import { Eye, EyeOff, FileText, Plus, TerminalSquare, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { isTauriRuntime, readTerminalSnapshot, resizeTerminal, startAcpSession, startTerminal, stopAcpSession, stopTerminal, writeTerminal } from "../lib/api";
import type { AcpEvent, AgentConfig, Session, SessionEvent, SessionStatus, SessionViewMode, TerminalOutput } from "../lib/types";

type SessionViewProps = {
  agent: AgentConfig;
  events: SessionEvent[];
  session: Session;
  viewMode: SessionViewMode;
  openSessions: Session[];
  onTabSelect: (session: Session) => void;
  onTabClose: (sessionId: string) => void;
  onNewTab: () => void;
  onSessionStatusChange?: (status: SessionStatus) => void;
  onViewModeToggle: () => void;
};

export function SessionView({ agent, events, session, viewMode, openSessions, onTabSelect, onTabClose, onNewTab, onSessionStatusChange, onViewModeToggle }: SessionViewProps) {
  const [detailsVisible, setDetailsVisible] = useState(false);
  return (
    <section className="session-view" aria-label="Session view">
      <div className="editor-tabs" role="tablist" aria-label="Session tabs">
        {openSessions.map((openSession) => (
          <div className={openSession.id === session.id ? "editor-tab is-active" : "editor-tab"} key={openSession.id} role="tab" aria-selected={openSession.id === session.id}>
            <button className="editor-tab__select" type="button" title={openSession.cwd} onClick={() => onTabSelect(openSession)}>
              <span>{openSession.label}</span>
            </button>
            <button className="editor-tab__close" type="button" title={`Close ${openSession.label}`} aria-label={`Close ${openSession.label}`} onClick={() => onTabClose(openSession.id)}>
              <X size={12} />
            </button>
          </div>
        ))}
        <button className="editor-tab__new" type="button" title="New conversation" aria-label="New conversation" onClick={onNewTab}>
          <Plus size={14} />
        </button>
        <button
          className="editor-toggle"
          type="button"
          title={viewMode === "raw-terminal" ? "Switch to Clean View" : "Switch to Raw Terminal"}
          aria-label={viewMode === "raw-terminal" ? "Switch to Clean View" : "Switch to Raw Terminal"}
          onClick={onViewModeToggle}
        >
          {viewMode === "raw-terminal" ? <FileText size={14} /> : <TerminalSquare size={14} />}
        </button>
      </div>

      {viewMode === "raw-terminal" ? (
        <RawTerminal agent={agent} session={session} onSessionStatusChange={onSessionStatusChange} />
      ) : (
        <CleanPlaceholder events={events} detailsVisible={detailsVisible} onDetailsToggle={() => setDetailsVisible((visible) => !visible)} />
      )}
    </section>
  );
}

function RawTerminal({ agent, session, onSessionStatusChange }: { agent: AgentConfig; session: Session; onSessionStatusChange?: (status: SessionStatus) => void }) {
  const reportStatus = useCallback((nextStatus: SessionStatus) => {
    onSessionStatusChange?.(nextStatus);
  }, [onSessionStatusChange]);

  return (
    <div className="terminal-frame">
      <div className="terminal-toolbar">
        <span>
          <TerminalSquare size={15} />
          {agent.label}
        </span>
      </div>
      {agent.transport === "acp" ? (
        <AcpTerminalSurface sessionId={session.id} cwd={session.cwd} command={agent.command} args={agent.args} onStatusChange={reportStatus} />
      ) : (
        <TerminalSurface sessionId={session.id} cwd={session.cwd} onStatusChange={reportStatus} />
      )}
    </div>
  );
}

function AcpTerminalSurface({ sessionId, cwd, command, args, onStatusChange }: { sessionId: string; cwd: string; command: string; args: string[]; onStatusChange: (status: SessionStatus) => void }) {
  const hostRef = useRef<HTMLDivElement>(null);

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
    const fit = () => fitAddon.fit();
    const resizeObserver = new ResizeObserver(fit);
    resizeObserver.observe(host);

    const setup = async () => {
      if (!isTauriRuntime()) {
        terminal.write("ACP requires the Tauri desktop runtime.\r\n");
        onStatusChange("error");
        return;
      }
      try {
        // Listen before start so session/ready and tool events are not missed
        unlisten = await listen<AcpEvent>("acp-event", (event) => {
          const payload = event.payload;
          if (payload.sessionId !== sessionId || disposed) return;
          const body = typeof payload.data === "string" ? payload.data : JSON.stringify(payload.data);
          // Keep terminal readable: short lines for large payloads
          const clipped = body.length > 1200 ? `${body.slice(0, 1200)}…` : body;
          terminal.write(`[${payload.method ?? payload.kind}] ${clipped}\r\n`);
          if (payload.kind === "system" && payload.method === "session/ready") onStatusChange("waiting");
        });

        terminal.write(`Starting ${command} ${args.join(" ")} …\r\n`);
        onStatusChange("starting");
        const caps = await startAcpSession(sessionId, command, args, cwd);
        if (disposed) return;

        if (caps) {
          const model = caps.currentModel ?? "(default)";
          const mode = caps.currentMode ?? "-";
          const modelCount = caps.models?.length ?? 0;
          const modeCount = caps.modes?.length ?? 0;
          terminal.write(
            `ACP ready · model=${model} (${modelCount} available) · mode=${mode} (${modeCount})\r\n`
          );
        } else {
          terminal.write("ACP ready\r\n");
        }

        if (!disposed) {
          onStatusChange("waiting");
          requestAnimationFrame(fit);
        }
      } catch (error) {
        terminal.write(`ACP could not start: ${String(error)}\r\n`);
        onStatusChange("error");
      }
    };
    void setup();
    requestAnimationFrame(fit);
    return () => {
      disposed = true;
      unlisten?.();
      resizeObserver.disconnect();
      terminal.dispose();
      if (isTauriRuntime()) void stopAcpSession(sessionId).catch(() => undefined);
    };
  }, [args, command, cwd, onStatusChange, sessionId]);

  return <div ref={hostRef} className="terminal-surface" aria-label="ACP session output" />;
}

function TerminalSurface({ sessionId, cwd, onStatusChange }: { sessionId: string; cwd: string; onStatusChange: (status: SessionStatus) => void }) {
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const style = getComputedStyle(document.documentElement);
    const terminal = new Terminal({
      convertEol: true,
      cursorBlink: true,
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
    const fitAndResize = () => {
      fitAddon.fit();
      if (isTauriRuntime() && terminal.cols > 0 && terminal.rows > 0) {
        void resizeTerminal(sessionId, terminal.cols, terminal.rows).catch(() => undefined);
      }
    };
    const resizeObserver = new ResizeObserver(fitAndResize);
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
        terminal.write("AgentShell M3 Raw Terminal requires the Tauri desktop runtime.\r\n");
        onStatusChange("error");
        return;
      }

      try {
        unlisten = await listen<TerminalOutput>("session-output", (event) => {
          const output = event.payload;
          if (output.sessionId !== sessionId || disposed) return;
          if (output.data) terminal.write(output.data);
          if (output.error) {
            terminal.write(`\r\n[terminal error] ${output.error}\r\n`);
            onStatusChange("error");
          }
          if (output.exited) onStatusChange("exited");
        });
        const snapshot = await readTerminalSnapshot(sessionId, cwd);
        if (snapshot && !disposed) terminal.write(snapshot);
        await startTerminal(sessionId, cwd);
        if (!disposed) {
          onStatusChange("running");
          requestAnimationFrame(fitAndResize);
        }
      } catch (error) {
        terminal.write(`AgentShell could not start the terminal: ${String(error)}\r\n`);
        onStatusChange("error");
      }
    };
    void setup();
    requestAnimationFrame(fitAndResize);

    return () => {
      disposed = true;
      unlisten?.();
      input.dispose();
      resizeObserver.disconnect();
      terminal.dispose();
    };
  }, [cwd, onStatusChange, sessionId]);

  return <div ref={hostRef} className="terminal-surface" aria-label="Raw terminal" />;
}

function CleanPlaceholder({ events, detailsVisible, onDetailsToggle }: { events: SessionEvent[]; detailsVisible: boolean; onDetailsToggle: () => void }) {
  return (
    <div className="clean-surface">
      <div className="clean-surface__notice">
        <FileText size={16} />
        Clean View placeholder. Parser-backed cards arrive after Raw Terminal is wired.
        <button className="icon-button icon-button--small" type="button" title={detailsVisible ? "Hide tool calls and thinking" : "Show tool calls and thinking"} onClick={onDetailsToggle}>
          {detailsVisible ? <Eye size={13} /> : <EyeOff size={13} />}
        </button>
      </div>

      <div className={detailsVisible ? "event-list" : "event-list is-details-hidden"}>
        {events.map((event) => {
          const body =
            event.type === "handoff_prepared"
              ? event.prompt
              : event.type === "file_change"
                ? `${event.changeType}: ${event.path}`
                : event.text;

          return (
            <article className={`event-card event-card--${event.type}`} key={`${event.type}-${event.createdAt}`}>
              <div className="event-card__icon">
                <FileText size={15} />
              </div>
              <div>
                <span className="event-card__type">{event.type.replace("_", " ")}</span>
                <p>{body}</p>
              </div>
            </article>
          );
        })}
      </div>
    </div>
  );
}
