import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import "./styles/tokens.css";
import "./styles/app.css";
import "@xterm/xterm/css/xterm.css";

/** Prevent a single component crash from turning the whole window pure black. */
class RootErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div
          style={{
            boxSizing: "border-box",
            height: "100vh",
            padding: 24,
            background: "#101214",
            color: "#edf1f5",
            fontFamily: "ui-sans-serif, system-ui, sans-serif",
          }}
        >
          <h1 style={{ margin: "0 0 8px", fontSize: 18 }}>Marionette UI crashed</h1>
          <p style={{ margin: "0 0 12px", color: "#84909d", fontSize: 13 }}>
            The shell hit a render error. Copy the message, then reload the window.
          </p>
          <pre
            style={{
              whiteSpace: "pre-wrap",
              padding: 12,
              borderRadius: 6,
              background: "#1b1f24",
              border: "1px solid #2a3037",
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
              border: "1px solid #3a424c",
              background: "#20252b",
              color: "#edf1f5",
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

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <RootErrorBoundary>
      <App />
    </RootErrorBoundary>
  </React.StrictMode>
);
