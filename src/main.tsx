import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import {
  installDiagnosticCapture,
  recordUiError,
} from "./lib/diagnosticCapture";
import {
  chiefProbeDrive,
  chiefProbeReplay,
  chiefProbeStop,
  isTauriRuntime,
} from "./lib/api";
import "./styles/tokens.css";
import "./styles/app.css";
import "./styles/motion.css";
import "./styles/dock.css";
import "./styles/usage-panel.css";
import "./styles/rail.css";
import "./styles/v2/shelf.css";
import "./styles/v2/transcript.css";
import "./styles/v2/composer.css";
import "./styles/v2/dialogs.css";
import "./styles/v2/chrome.css";
import "./styles/v2/memo.css";
import "./styles/v2/council.css";
import "./styles/v2/dock.css";
import "./styles/v2/usage.css";
import "./styles/usage-v2/base.css";
import "./styles/usage-v2/hero.css";
import "./styles/usage-v2/where.css";
import "./styles/usage-v2/agents.css";
import "./styles/usage-v2/activity.css";
import "./styles/usage-v2/sessions.css";
import "./styles/seen-images/base.css";
import "./styles/seen-images/rail.css";
import "./styles/seen-images/viewer.css";
import "./styles/v2/pages.css";
import "./styles/chief.css";
import "./styles/staff-chat.css";

/** Prevent a single component crash from turning the whole window pure black. */
class RootErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    recordUiError({
      source: "ui-react-boundary",
      message: error.message || "React root boundary caught an error",
      detail: [error.stack, info.componentStack].filter(Boolean).join("\n\n"),
    });
  }

  render() {
    if (this.state.error) {
      return (
        <div
          style={{
            boxSizing: "border-box",
            height: "100vh",
            padding: 24,
            background: "#1e2329",
            color: "#f0f3f6",
            fontFamily: "ui-sans-serif, system-ui, sans-serif",
          }}
        >
          <h1 style={{ margin: "0 0 8px", fontSize: 18 }}>Marionette UI crashed</h1>
          <p style={{ margin: "0 0 12px", color: "#8b97a4", fontSize: 13 }}>
            The shell hit a render error. Copy the message, then reload the window.
          </p>
          <pre
            style={{
              whiteSpace: "pre-wrap",
              padding: 12,
              borderRadius: 6,
              background: "#2c333b",
              border: "1px solid #3d4650",
              color: "#ef6c73",
              fontSize: 12,
            }}
          >
            {this.state.error.message}
            {"\n\n"}
            {this.state.error.stack}
          </pre>
          <button
            type="button"
            style={{
              marginTop: 16,
              padding: "8px 14px",
              borderRadius: 6,
              border: "1px solid #515b67",
              background: "#343c45",
              color: "#f0f3f6",
              cursor: "pointer",
            }}
            onClick={() => window.location.reload()}
          >
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

installDiagnosticCapture();

// tauri 2.11.5 注入的 `unregisterListener` 对「条目已不存在」直接读
// `listeners[eventId].handlerId` 抛 TypeError——任何一次重复 unlisten /
// once() 竞态都以 unhandled rejection 收场（dev.log 里成串的
// `Cannot read properties of undefined (reading 'handlerId')`）。
// 上游 2.12.x 已加 `if (listener)` 守卫；升级 tauri 越过修复版后删除本垫片。
{
  const internals = (
    window as unknown as {
      __TAURI_EVENT_PLUGIN_INTERNALS__?: {
        unregisterListener?: (event: string, eventId: number) => void;
        __marionetteGuarded?: boolean;
      };
    }
  ).__TAURI_EVENT_PLUGIN_INTERNALS__;
  if (
    internals &&
    typeof internals.unregisterListener === "function" &&
    !internals.__marionetteGuarded
  ) {
    internals.__marionetteGuarded = true;
    const original = internals.unregisterListener;
    internals.unregisterListener = (event, eventId) => {
      try {
        original(event, eventId);
      } catch {
        // Entry already removed — the follow-up `plugin:event|unlisten`
        // invoke in _unlisten is a harmless no-op on the Rust side.
      }
    };
  }
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <RootErrorBoundary>
      <App />
    </RootErrorBoundary>
  </React.StrictMode>
);

// Chief 探针入口（M0 诊断面，非产品 UI）：devtools 里
// `await window.__chiefProbe.drive({ agentId: "codex", cwd: "...", prompt: "..." })`。
// 主窗隐藏/最小化时回合照跑，事件 tee 落 `~/.marionette/chief/sessions/`；
// 重启后 `replay(sessionId)` 回放。ChiefBroker 落地后由真正的驱动路径替换。
if (isTauriRuntime()) {
  (window as unknown as Record<string, unknown>).__chiefProbe = {
    drive: chiefProbeDrive,
    stop: chiefProbeStop,
    replay: chiefProbeReplay,
  };
}
