import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowRightToLine, Minus, Square, X } from "lucide-react";
import { useCallback, useEffect, useState, type MouseEvent } from "react";
import { isTauriRuntime } from "../lib/api";
import { enterDock } from "../lib/dockMode";
import { useT } from "../lib/i18n";

type WindowControlsProps = {
  className?: string;
};

/**
 * Frameless chrome buttons. Must NOT sit under a parent with
 * `data-tauri-drag-region` — drag steals mousedown and clicks never fire.
 */
export function WindowControls({ className = "" }: WindowControlsProps) {
  const t = useT();
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

  /**
   * 贴边专属按钮（2026-10-31）：× 不再承担贴边（后台模式点 × = 隐藏驻留），
   * 进贴边只走这个按钮和拖到屏幕边缘吸附。dock 态下整组 window-controls
   * 被 dock.css 藏起，不会出现「已贴边还能再贴」。
   */
  const dockToEdge = useCallback((event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (!isTauriRuntime()) return;
    void enterDock();
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
      aria-label={t("v2chrome.winControlsAria")}
      onMouseDown={guard}
      onPointerDown={guard}
    >
      <button
        className="window-controls__btn window-controls__btn--dock"
        type="button"
        title={t("v2chrome.dockToEdge")}
        aria-label={t("v2chrome.dockToEdge")}
        onMouseDown={guard}
        onClick={dockToEdge}
      >
        <ArrowRightToLine size={14} strokeWidth={1.5} />
      </button>
      <button
        className="window-controls__btn"
        type="button"
        title={t("v2chrome.minimize")}
        aria-label={t("v2chrome.minimize")}
        onMouseDown={guard}
        onClick={minimize}
      >
        <Minus size={14} strokeWidth={1.5} />
      </button>
      <button
        className="window-controls__btn"
        type="button"
        title={maximized ? t("v2chrome.restore") : t("v2chrome.maximize")}
        aria-label={maximized ? t("v2chrome.restore") : t("v2chrome.maximize")}
        onMouseDown={guard}
        onClick={toggleMax}
      >
        <Square size={14} strokeWidth={1.5} />
      </button>
      <button
        className="window-controls__btn window-controls__btn--close"
        type="button"
        title={t("v2chrome.close")}
        aria-label={t("v2chrome.close")}
        onMouseDown={guard}
        onClick={close}
      >
        <X size={14} strokeWidth={1.5} />
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
