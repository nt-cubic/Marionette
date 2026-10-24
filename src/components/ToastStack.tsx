import { useEffect, useState, type CSSProperties } from "react";
import { X } from "lucide-react";
import { dismissToast, subscribeToasts, type Toast } from "../lib/toast";

const VISIBLE = 3;

function ToastItem({ toast, paused, depth }: { toast: Toast; paused: boolean; depth: number }) {
  // Remaining life survives pauses, so hovering never resets the clock.
  const [left, setLeft] = useState(toast.duration);
  useEffect(() => {
    if (paused) return;
    const started = Date.now();
    const id = window.setTimeout(() => dismissToast(toast.id), left);
    return () => {
      window.clearTimeout(id);
      setLeft((ms) => Math.max(0, ms - (Date.now() - started)));
    };
  }, [paused, toast.id]);
  return (
    <div
      className={`toast is-${toast.tone}`}
      role={toast.tone === "error" ? "alert" : "status"}
      style={{ "--toast-depth": depth } as CSSProperties}
    >
      <span className="toast__dot" aria-hidden />
      <div className="toast__copy">
        <span className="toast__text">{toast.text}</span>
        {toast.detail && <span className="toast__detail">{toast.detail}</span>}
      </div>
      {toast.action && (
        <button
          type="button"
          className="toast__action"
          onClick={() => {
            dismissToast(toast.id);
            toast.action?.run();
          }}
        >
          {toast.action.label}
        </button>
      )}
      <button type="button" className="toast__close" aria-label="关闭" onClick={() => dismissToast(toast.id)}>
        <X size={11} />
      </button>
    </div>
  );
}

/**
 * Bottom-right result stack: the newest in front, older ones tucked behind a
 * few pixels each; pointing at the stack fans it out and pauses every timer.
 */
export function ToastStack() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [hover, setHover] = useState(false);
  useEffect(() => subscribeToasts(setToasts), []);
  if (toasts.length === 0) return null;
  const shown = toasts.slice(-VISIBLE);
  return (
    <div
      className={`toast-stack${hover ? " is-fanned" : ""}`}
      aria-live="polite"
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      {shown.map((toast, i) => (
        <ToastItem key={toast.id} toast={toast} paused={hover} depth={shown.length - 1 - i} />
      ))}
    </div>
  );
}
