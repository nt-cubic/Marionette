import { listen } from "@tauri-apps/api/event";
import { Circle, Expand, Plus, SendHorizontal, Square, Target } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { getSessionCapabilities, isTauriRuntime, updateAcpSession } from "../lib/api";
import type { AcpEvent, AgentConfig, CapabilitySnapshot, SessionStatus } from "../lib/types";

type ComposerProps = {
  agent: AgentConfig;
  agents: AgentConfig[];
  currentAgentId: string;
  sessionId: string;
  sessionStatus: SessionStatus;
  onAgentChange: (agentId: string) => void;
  onInterrupt: () => void;
  onSend: (text: string) => void;
};

function effortLabel(value: number): string {
  if (value <= 0.1) return "Low";
  if (value >= 0.9) return "High";
  if (Math.abs(value - 0.5) < 0.1) return "Auto";
  return value < 0.5 ? "Medium" : "High";
}

export function Composer({ agent, agents, currentAgentId, sessionId, sessionStatus, onAgentChange, onInterrupt, onSend }: ComposerProps) {
  const [caps, setCaps] = useState<CapabilitySnapshot | null>(null);
  const [currentMode, setCurrentMode] = useState<string | null>(null);
  const [currentModel, setCurrentModel] = useState<string | null>(null);
  const [currentEffort, setCurrentEffort] = useState<number | null>(null);
  const [menu, setMenu] = useState<"mode" | "model" | "effort" | null>(null);
  const [draft, setDraft] = useState("");
  const [flashError, setFlashError] = useState("");
  const errorTimer = useRef<ReturnType<typeof setTimeout>>();
  const updating = useRef(false);

  // ── Shared capability loader ────────────────────────────────────────────────
  const loadCapabilities = useCallback((id: string) => {
    getSessionCapabilities(id).then((result) => {
      setCaps(result);
      if (result) {
        setCurrentMode(result.currentMode);
        setCurrentModel(result.currentModel);
        if (result.currentEffort != null) {
          setCurrentEffort(result.currentEffort);
        } else if (result.thinkingEffort) {
          setCurrentEffort(result.thinkingEffort.default);
        }
      }
    });
  }, []);

  // ── Fetch capabilities when session changes or becomes ready ───────────────
  useEffect(() => {
    if (!sessionId || sessionId.startsWith("session-empty-")) {
      setCaps(null);
      setCurrentMode(null);
      setCurrentModel(null);
      setCurrentEffort(null);
      return;
    }
    // Reload once the ACP process reports waiting/running so we never miss
    // session/ready races during start_acp_session.
    if (sessionStatus === "waiting" || sessionStatus === "running" || sessionStatus === "starting") {
      loadCapabilities(sessionId);
    } else {
      setCaps(null);
    }
  }, [sessionId, sessionStatus, loadCapabilities]);

  // ── Also re-fetch on ACP session/ready (and permission auto-allow logs) ────
  useEffect(() => {
    if (!sessionId || sessionId.startsWith("session-empty-") || !isTauriRuntime()) return;

    let unlisten: (() => void) | undefined;
    listen<AcpEvent>("acp-event", (event) => {
      const payload = event.payload;
      if (payload.sessionId !== sessionId) return;
      if (payload.kind === "system" && payload.method === "session/ready") {
        const data = payload.data as { capabilities?: CapabilitySnapshot } | null;
        if (data?.capabilities) {
          setCaps(data.capabilities);
          setCurrentMode(data.capabilities.currentMode);
          setCurrentModel(data.capabilities.currentModel);
          if (data.capabilities.currentEffort != null) {
            setCurrentEffort(data.capabilities.currentEffort);
          } else if (data.capabilities.thinkingEffort) {
            setCurrentEffort(data.capabilities.thinkingEffort.default);
          }
        } else {
          loadCapabilities(sessionId);
        }
      }
    }).then((dispose) => {
      unlisten = dispose;
    });
    return () => unlisten?.();
  }, [sessionId, loadCapabilities]);

  // ── Update helpers ─────────────────────────────────────────────────────────
  const flash = useCallback((msg: string) => {
    setFlashError(msg);
    if (errorTimer.current) clearTimeout(errorTimer.current);
    errorTimer.current = setTimeout(() => setFlashError(""), 3000);
  }, []);

  const commitUpdate = useCallback(
    async (patch: Record<string, unknown>, revert: () => void) => {
      if (updating.current || !sessionId) return;
      updating.current = true;
      try {
        await updateAcpSession(sessionId, patch);
        // Refresh from agent so current values stay honest after set_config_option
        loadCapabilities(sessionId);
      } catch {
        revert();
        flash("Failed to update");
      } finally {
        updating.current = false;
      }
    },
    [sessionId, flash, loadCapabilities],
  );

  // ── Send / Interrupt ───────────────────────────────────────────────────────
  const isRunning = sessionStatus === "starting" || sessionStatus === "running";
  const submit = () => {
    const text = draft.trim();
    if (!text || isRunning) return;
    onSend(text);
    setDraft("");
  };

  // ── Derived state ──────────────────────────────────────────────────────────
  // Modes: only offer switcher when Agent exposes more than one.
  // Models: show whenever the list is non-empty so the current model is visible.
  const hasModes = caps != null && caps.modes.length > 1;
  const hasModels = caps != null && caps.models.length > 0;
  const hasEffort = caps != null && caps.thinkingEffort != null && caps.effortConfigId != null;

  const displayModel =
    currentModel ?? (caps && caps.models.length > 0 ? caps.models[0].id : null);
  const displayMode =
    currentMode ?? (caps && caps.modes.length > 0 ? caps.modes[0].id : null);
  const displayEffort =
    hasEffort && currentEffort != null ? effortLabel(currentEffort) : null;
  const displayModelLabel =
    (displayModel && caps?.models.find((m) => m.id === displayModel)?.label) || displayModel;
  const displayModeLabel =
    (displayMode && caps?.modes.find((m) => m.id === displayMode)?.label) || displayMode;

  return (
    <footer className="composer" aria-label="Composer">
      {flashError && <div className="composer__error-toast">{flashError}</div>}
      <div className="composer__field">
        <button className="composer-expand" type="button" title="Expand composer">
          <Expand size={13} />
        </button>
        <textarea
          aria-label="Prompt composer"
          placeholder="Message the Agent, @ to include context, / for commands"
          rows={2}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
              event.preventDefault();
              submit();
            }
          }}
        />
        <div className="composer__toolbar">
          <div className="composer__controls">
            <button className="composer-tool" type="button" title="Add files or context">
              <Plus size={14} />
            </button>
          </div>
          <div className="composer__actions">
            {/* ── Mode selector (only if agent exposes ≥2 modes) ──────── */}
            {hasModes && displayMode && (
              <div className="composer-menu-anchor">
                <button
                  className="composer-select"
                  type="button"
                  title="Execution mode"
                  aria-expanded={menu === "mode"}
                  onClick={() => setMenu(menu === "mode" ? null : "mode")}
                >
                  <Target size={13} />
                  {displayModeLabel}
                </button>
                {menu === "mode" && (
                  <div className="composer-menu" role="menu" aria-label="Execution mode">
                    {caps!.modes.map((m) => (
                      <button
                        key={m.id}
                        className={displayMode === m.id ? "is-selected" : ""}
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          const prev = displayMode;
                          setCurrentMode(m.id);
                          setMenu(null);
                          void commitUpdate(
                            { mode: m.id },
                            () => setCurrentMode(prev),
                          );
                        }}
                      >
                        {m.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}

            {/* ── Model + Effort selector ─────────────────────────────── */}
            {(hasModels || hasEffort) && (
              <div className="composer-menu-anchor">
                <button
                  className="composer-select composer-select--model"
                  type="button"
                  title="Choose model and response strength"
                  aria-expanded={menu === "model" || menu === "effort"}
                  onClick={() => setMenu(menu === "model" ? null : "model")}
                >
                  {displayModelLabel ?? "default"}
                  {displayEffort != null && hasEffort ? ` · ${displayEffort}` : ""}
                </button>

                {menu === "model" && (
                  <div className="composer-menu" role="menu" aria-label={`${agent.label} models`}>
                    {hasModels && (
                      <>
                        <span className="composer-menu__label">{agent.label} model</span>
                        {caps!.models.map((m) => (
                          <button
                            key={m.id}
                            className={displayModel === m.id ? "is-selected" : ""}
                            type="button"
                            role="menuitem"
                            onClick={() => {
                              const prev = displayModel;
                              setCurrentModel(m.id);
                              // Move to effort selection if available
                              if (hasEffort) {
                                setMenu("effort");
                              } else {
                                setMenu(null);
                              }
                              void commitUpdate(
                                { model: m.id },
                                () => setCurrentModel(prev),
                              );
                            }}
                          >
                            {m.label}
                          </button>
                        ))}
                        {hasEffort && <span className="composer-menu__divider" />}
                      </>
                    )}
                    {hasEffort && (
                      <>
                        <span className="composer-menu__label">Response strength</span>
                        {[
                          { label: "Auto", value: caps!.thinkingEffort!.default },
                          { label: "Low", value: caps!.thinkingEffort!.min },
                          { label: "High", value: caps!.thinkingEffort!.max },
                        ].map((preset) => (
                          <button
                            key={preset.label}
                            className={
                              currentEffort != null &&
                              Math.abs(currentEffort - preset.value) < 0.01
                                ? "is-selected"
                                : ""
                            }
                            type="button"
                            role="menuitem"
                            onClick={() => {
                              const prev = currentEffort;
                              setCurrentEffort(preset.value);
                              setMenu(null);
                              void commitUpdate(
                                { thinkingEffort: preset.value },
                                () => setCurrentEffort(prev),
                              );
                            }}
                          >
                            {preset.label} ({preset.value.toFixed(1)})
                          </button>
                        ))}
                      </>
                    )}
                  </div>
                )}

                {menu === "effort" && (
                  <div className="composer-menu" role="menu" aria-label="Response strength">
                    <span className="composer-menu__label">Response strength</span>
                    {[
                      { label: "Auto", value: caps!.thinkingEffort!.default },
                      { label: "Low", value: caps!.thinkingEffort!.min },
                      { label: "High", value: caps!.thinkingEffort!.max },
                    ].map((preset) => (
                      <button
                        key={preset.label}
                        className={
                          currentEffort != null &&
                          Math.abs(currentEffort - preset.value) < 0.01
                            ? "is-selected"
                            : ""
                        }
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          const prev = currentEffort;
                          setCurrentEffort(preset.value);
                          setMenu(null);
                          void commitUpdate(
                            { thinkingEffort: preset.value },
                            () => setCurrentEffort(prev),
                          );
                        }}
                      >
                        {preset.label} ({preset.value.toFixed(1)})
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}

            {/* ── Agent switcher ──────────────────────────────────────── */}
            <label className="composer-select composer-select--agent" title="Switch Agent">
              <select
                aria-label="Switch Agent"
                value={currentAgentId}
                onChange={(event) => {
                  setMenu(null);
                  onAgentChange(event.target.value);
                }}
              >
                {agents
                  .filter((candidate) => candidate.enabled)
                  .map((candidate) => (
                    <option key={candidate.id} value={candidate.id}>
                      {candidate.label}
                    </option>
                  ))}
              </select>
            </label>

            {/* ── Send / Interrupt ────────────────────────────────────── */}
            <button
              className={
                isRunning ? "send-button is-interrupting" : "send-button"
              }
              type="button"
              title={isRunning ? "Interrupt conversation" : "Send"}
              aria-label={isRunning ? "Interrupt conversation" : "Send"}
              onClick={() => {
                if (isRunning) onInterrupt();
                else submit();
              }}
            >
              {isRunning ? (
                <Square size={12} fill="currentColor" />
              ) : (
                <SendHorizontal size={15} />
              )}
            </button>
          </div>
        </div>
      </div>
    </footer>
  );
}
