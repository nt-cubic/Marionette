import { useEffect, useState } from "react";
import { Download, Eye, MousePointerClick, RefreshCw, ShieldAlert, X } from "lucide-react";
import {
  computerInstallDriver,
  computerListShareable,
  computerRevokeAll,
  computerShare,
  onDriverProgress,
  type DriverProgress,
  type ShareableState,
} from "../lib/computer";
import { useT } from "../lib/i18n";

/**
 * The Computer Use share panel (Ctrl K → 共享窗口给 agent). Everything
 * human-side lives here: the engine install (35MB, pinned download), the
 * window picker with per-window 只读/操作 grants, and Stop (also on the
 * global Ctrl+Alt+Shift+S shortcut). Agent-initiated requests do NOT come
 * here — they ride the in-conversation permission card like browser Control.
 */
export function ComputerShareDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  const [state, setState] = useState<ShareableState | null>(null);
  const [progress, setProgress] = useState<DriverProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = () => {
    void computerListShareable().then(setState);
  };

  useEffect(() => {
    refresh();
    const stop = onDriverProgress((update) => {
      setProgress(update);
      if (update.phase === "installed" || update.phase === "failed") {
        if (update.phase === "failed") setError(t("computer.installFailed"));
        setTimeout(refresh, 300);
      }
    });
    return () => {
      void stop.then((unlisten) => unlisten());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const install = () => {
    setBusy(true);
    setError(null);
    setProgress({ phase: "downloading", downloaded: 0 });
    void computerInstallDriver()
      .then(() => {
        // The invoke resolving is success regardless of the event race;
        // clear the progress and re-read the state (covers the
        // already-installed early return that emits late or not at all).
        setProgress(null);
        refresh();
      })
      .catch((err) => {
        setError(String(err));
        setProgress(null);
      })
      .finally(() => setBusy(false));
  };

  const share = (window: ShareableState["windows"][number], level: "read" | "control") => {
    setError(null);
    void computerShare(
      {
        kind: "window",
        pid: window.pid,
        startedAtMs: window.processStartedAtMs ?? 0,
        windowId: window.windowId,
      },
      level,
    )
      .then(refresh)
      .catch((err) => setError(String(err)));
  };

  const stopAll = () => {
    void computerRevokeAll().then(refresh);
  };

  const driver = state?.driver;
  const installed = driver?.installed ?? false;
  const downloading = progress?.phase === "downloading";

  return (
    <div className="project-dialog-backdrop" role="presentation" onClick={onClose}>
      <div className="computer-dialog" role="dialog" aria-modal onClick={(e) => e.stopPropagation()}>
        <div className="computer-dialog__head">
          <div className="computer-dialog__title">{t("computer.title")}</div>
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm"
            title={t("common.close")}
            onClick={onClose}
          >
            <X size={13} />
          </button>
        </div>

        <div className="computer-dialog__hint">{t("computer.hint")}</div>

        {!installed && (
          <div className="computer-dialog__install">
            <div className="computer-dialog__install-text">{t("computer.installIntro")}</div>
            {downloading ? (
              <div className="computer-dialog__progress">
                {progress?.total
                  ? t("computer.progressMb", {
                      done: Math.floor((progress.downloaded ?? 0) / 1024 / 1024),
                      total: Math.floor(progress.total / 1024 / 1024),
                    })
                  : t("computer.progressIndeterminate")}
              </div>
            ) : (
              <button type="button" className="project-dialog__submit" disabled={busy} onClick={install}>
                <Download size={13} />
                {t("computer.installButton")}
              </button>
            )}
          </div>
        )}

        {(error || state?.helperError) && (
          <div className="computer-dialog__error" role="alert">
            <ShieldAlert size={13} />
            <span>{error ?? state?.helperError}</span>
          </div>
        )}

        {installed && (
          <div className="computer-dialog__list">
            {(state?.windows ?? [])
              .filter((window) => window.grantable)
              .map((window) => (
                <div key={`${window.pid}:${window.windowId}`} className="computer-window">
                  <div className="computer-window__main">
                    <div className="computer-window__title" title={window.processImage ?? ""}>
                      {window.title || t("computer.untitledWindow")}
                    </div>
                    <div className="computer-window__sub">
                      {(window.processImage ?? "").split(/[\\/]/).pop()}
                      {window.minimized ? ` · ${t("computer.minimized")}` : ""}
                      {window.shared !== "none" ? ` · ${t(`computer.shared.${window.shared}`)}` : ""}
                    </div>
                  </div>
                  <div className="computer-window__actions">
                    <button
                      type="button"
                      className={`computer-dialog__text-btn${window.shared === "read" ? " is-active" : ""}`}
                      title={t("computer.shareReadHint")}
                      onClick={() => share(window, "read")}
                    >
                      <Eye size={12} />
                      {t("computer.shareRead")}
                    </button>
                    <button
                      type="button"
                      className={`computer-dialog__text-btn${window.shared === "control" ? " is-active" : ""}`}
                      title={t("computer.shareControlHint")}
                      onClick={() => share(window, "control")}
                    >
                      <MousePointerClick size={12} />
                      {t("computer.shareControl")}
                    </button>
                  </div>
                </div>
              ))}
            {(state?.windows ?? []).filter((window) => window.grantable).length === 0 && (
              <div className="computer-dialog__empty">{t("computer.noWindows")}</div>
            )}
          </div>
        )}

        <div className="computer-dialog__foot">
          <span className="computer-dialog__shortcut">{t("computer.stopShortcutHint")}</span>
          <div className="computer-dialog__foot-actions">
            <button type="button" className="computer-dialog__text-btn" onClick={refresh}>
              <RefreshCw size={12} />
              {t("computer.refresh")}
            </button>
            <button type="button" className="project-dialog__submit project-dialog__submit--danger" onClick={stopAll}>
              {t("computer.stopAll")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
