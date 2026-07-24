import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";
import { useCallback, useEffect, useState, type MouseEvent } from "react";
import { isTauriRuntime } from "../lib/api";

type WindowControlsProps = {
  className?: string;
};

/**
 * Frameless chrome buttons. Must NOT sit under a parent with
 * `data-tauri-drag-region` — drag steals mousedown and clicks never fire.
 */
export function WindowControls({ className = "" }: WindowControlsProps) {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    try {
      const win = getCurrentWindow();
      void win.isMaximized().then((isMax) => {
        if (!disposed) setMaximized(isMax);
      });
      void win.onResized(() => {
        void win.isMaximized().then((next) => {
          if (!disposed) setMaximized(next);
        });
      }).then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      });
    } catch {
      // browser preview
    }
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** Stop parent drag-region / titlebar handlers from eating the gesture. */
  const guard = useCallback((event: MouseEvent) => {
    event.stopPropagation();
  }, []);

  const minimize = useCallback((event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (!isTauriRuntime()) return;
    try {
      void getCurrentWindow().minimize();
    } catch (error) {
      console.error("window.minimize failed", error);
    }
  }, []);

  const toggleMax = useCallback((event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (!isTauriRuntime()) return;
    try {
      const win = getCurrentWindow();
      void win.toggleMaximize().then(async () => {
        setMaximized(await win.isMaximized());
      });
    } catch (error) {
      console.error("window.toggleMaximize failed", error);
    }
  }, []);

  const close = useCallback((event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (!isTauriRuntime()) return;
    try {
      void getCurrentWindow().close();
    } catch (error) {
      console.error("window.close failed", error);
    }
  }, []);

  if (!isTauriRuntime()) return null;

  return (
    <div
      className={`window-controls ${className}`.trim()}
      role="group"
      aria-label="Window controls"
      onMouseDown={guard}
      onPointerDown={guard}
    >
      <button
        className="window-controls__btn"
        type="button"
        title="Minimize"
        aria-label="Minimize"
        onMouseDown={guard}
        onClick={minimize}
      >
        <Minus size={12} strokeWidth={2.2} />
      </button>
      <button
        className="window-controls__btn"
        type="button"
        title={maximized ? "Restore" : "Maximize"}
        aria-label={maximized ? "Restore" : "Maximize"}
        onMouseDown={guard}
        onClick={toggleMax}
      >
        <Square size={10} strokeWidth={2.2} />
      </button>
      <button
        className="window-controls__btn window-controls__btn--close"
        type="button"
        title="Close"
        aria-label="Close"
        onMouseDown={guard}
        onClick={close}
      >
        <X size={12} strokeWidth={2.2} />
      </button>
    </div>
  );
}

/** Double-click maximize helper for drag regions. */
export function useTitlebarDoubleClick() {
  return useCallback((event: MouseEvent) => {
    if (!isTauriRuntime()) return;
    if (event.detail === 2) {
      try {
        void getCurrentWindow().toggleMaximize();
      } catch {
        // ignore
      }
    }
  }, []);
}
