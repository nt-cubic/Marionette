import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { Camera, Check, Keyboard, Loader2, RefreshCw, ShieldAlert, X } from "lucide-react";
import {
  DEFAULT_APP_SHORTCUTS,
  describeShortcut,
  readCaptureShortcut,
  resetCaptureShortcut,
  shortcutFromKeyboardEvent,
  validateCaptureShortcut,
  writeCaptureShortcut,
  type ShortcutAction,
  type ShortcutDescriptor,
  type ShortcutValidation,
} from "../lib/captureShortcut";
import {
  captureWindowIntoDraft,
  loadCapturableWindows,
  windowCaptureKey,
  windowProcessName,
  type CaptureDraftTarget,
  type CapturableWindow,
  type WindowCaptureBackend,
  type WindowCaptureDraftPayload,
  type WindowCaptureFailure,
  type WindowCaptureListResult,
  type WindowCaptureWarning,
} from "../lib/windowCapture";
import { useT } from "../lib/i18n";
import "./WindowCaptureDialog.css";

export type WindowCaptureDialogProps = {
  open: boolean;
  onClose: () => void;
  /**
   * App hand-off: put `payload.attachments` and `payload.text` into the current
   * composer (or a newly created chat when target === "new"). Must never send.
   */
  onDraftReady: (payload: WindowCaptureDraftPayload) => void | Promise<void>;
  defaultTarget?: CaptureDraftTarget;
  canUseCurrentDraft?: boolean;
  backend?: WindowCaptureBackend;
  shortcut?: ShortcutDescriptor;
  /** Live App chord registry; defaults to the chords mirrored in captureShortcut.ts. */
  shortcutRegistry?: readonly ShortcutAction[];
  onShortcutChange?: (descriptor: ShortcutDescriptor) => void;
};

function unknownErrorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

export function WindowCaptureDialog({
  open,
  onClose,
  onDraftReady,
  defaultTarget,
  canUseCurrentDraft = true,
  backend,
  shortcut: shortcutOverride,
  shortcutRegistry = DEFAULT_APP_SHORTCUTS,
  onShortcutChange,
}: WindowCaptureDialogProps) {
  const t = useT();
  const [list, setList] = useState<WindowCaptureListResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [target, setTarget] = useState<CaptureDraftTarget>(
    defaultTarget ?? (canUseCurrentDraft ? "current" : "new"),
  );
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<WindowCaptureFailure | null>(null);
  const [lastPayload, setLastPayload] = useState<WindowCaptureDraftPayload | null>(null);

  const [shortcut, setShortcut] = useState<ShortcutDescriptor>(
    () => shortcutOverride ?? readCaptureShortcut(),
  );
  const [recording, setRecording] = useState(false);
  const [shortcutIssue, setShortcutIssue] = useState<ShortcutValidation | null>(null);
  const shortcutValidation = useMemo(
    () => validateCaptureShortcut(shortcut, shortcutRegistry),
    [shortcut, shortcutRegistry],
  );

  useEffect(() => {
    if (shortcutOverride) setShortcut(shortcutOverride);
  }, [shortcutOverride]);

  useEffect(() => {
    if (open && !canUseCurrentDraft && target === "current") setTarget("new");
  }, [open, canUseCurrentDraft, target]);

  const refresh = useCallback(async () => {
    setLoading(true);
    const result = await loadCapturableWindows();
    setList(result);
    setLoading(false);
    if (!result.ok) {
      setSelectedKey(null);
      return;
    }
    setSelectedKey((current) => {
      if (current && result.windows.some((window) => windowCaptureKey(window) === current)) {
        return current;
      }
      const first = result.windows.find((window) => !window.minimized);
      return first ? windowCaptureKey(first) : null;
    });
  }, []);

  useEffect(() => {
    if (!open) {
      setRecording(false);
      setShortcutIssue(null);
      return;
    }
    void refresh();
  }, [open, refresh]);

  useEffect(() => {
    if (!open || recording) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, recording, onClose]);

  useEffect(() => {
    if (!open || !recording) return;
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();
      if (event.key === "Escape") {
        setRecording(false);
        return;
      }
      const next = shortcutFromKeyboardEvent(event);
      if (!next) return;
      const validation = writeCaptureShortcut(next, { registry: shortcutRegistry });
      if (!validation.ok) {
        setShortcutIssue(validation);
        return;
      }
      setShortcut(validation.descriptor);
      setShortcutIssue(null);
      setRecording(false);
      onShortcutChange?.(validation.descriptor);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, recording, onShortcutChange, shortcutRegistry]);

  if (!open) return null;

  const windows: CapturableWindow[] = list?.ok ? list.windows : [];
  const selected = windows.find((window) => windowCaptureKey(window) === selectedKey) ?? null;
  const listFailure = list && !list.ok ? list : null;
  const activeShortcutIssue = shortcutIssue ?? (shortcutValidation.ok ? null : shortcutValidation);

  const shortcutReason = (validation: ShortcutValidation): string => {
    if (validation.ok) return "";
    if (validation.reason === "conflict") {
      const action = validation.conflict ? t(validation.conflict.labelKey) : "";
      return t("capture.shortcut.reason.conflict", { action });
    }
    if (validation.reason === "needs-modifier") return t("capture.shortcut.reason.needsModifier");
    if (validation.reason === "reserved") return t("capture.shortcut.reason.reserved");
    return t("capture.shortcut.reason.empty");
  };

  const resetShortcut = () => {
    const validation = resetCaptureShortcut({ registry: shortcutRegistry });
    if (validation.ok) {
      setShortcut(validation.descriptor);
      setShortcutIssue(null);
      onShortcutChange?.(validation.descriptor);
      return;
    }
    setShortcutIssue(validation);
  };

  const warningText = (warning: WindowCaptureWarning): string => {
    if (warning === "summary-unavailable") return t("capture.warn.summaryUnavailable");
    if (warning === "summary-empty") return t("capture.warn.summaryEmpty");
    if (warning === "summary-truncated") return t("capture.warn.summaryTruncated");
    return t("capture.warn.windowShared");
  };

  const capture = async () => {
    if (!selected || selected.minimized || busy) return;
    const capturedTarget = target;
    setBusy(true);
    setFailure(null);
    setLastPayload(null);
    const result = await captureWindowIntoDraft(selected, { target: capturedTarget, backend });
    if (!result.ok) {
      setFailure(result);
      setBusy(false);
      return;
    }
    try {
      await onDraftReady(result.payload);
    } catch (error) {
      const reason = unknownErrorText(error);
      setFailure({
        ok: false,
        kind: "draft-unavailable",
        message: t("capture.error.draftUnavailable", { reason }),
        retryable: true,
        cause: reason,
      });
      setBusy(false);
      return;
    }
    setLastPayload(result.payload);
    setBusy(false);
  };

  const pick = (window: CapturableWindow) => {
    setSelectedKey(windowCaptureKey(window));
    setFailure(null);
    setLastPayload(null);
  };

  const chooseTarget = (next: CaptureDraftTarget) => {
    setTarget(next);
    setLastPayload(null);
    setFailure(null);
  };

  if (typeof document === "undefined") return null;

  // Portal to <body>: a capture opened from the edge-dock chrome must escape
  // `.dock-chrome`'s stacking context (z-index 40) or the dialog renders under
  // the dock's page overlays.
  return createPortal(
    <div className="project-dialog-backdrop" role="presentation" onClick={onClose}>
      <div
        className="computer-dialog window-capture"
        role="dialog"
        aria-modal
        aria-label={t("capture.title")}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="computer-dialog__head">
          <div className="computer-dialog__title">{t("capture.title")}</div>
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm"
            title={t("common.close")}
            onClick={onClose}
          >
            <X size={13} />
          </button>
        </div>

        <div className="computer-dialog__hint">{t("capture.hint")}</div>
        <div className="window-capture__privacy">{t("capture.privacy")}</div>

        <div className="window-capture__targets" role="group" aria-label={t("capture.title")}>
          <button
            type="button"
            className={`window-capture__target${target === "current" ? " is-active" : ""}`}
            aria-pressed={target === "current"}
            disabled={!canUseCurrentDraft || busy}
            onClick={() => chooseTarget("current")}
          >
            {t("capture.target.current")}
          </button>
          <button
            type="button"
            className={`window-capture__target${target === "new" ? " is-active" : ""}`}
            aria-pressed={target === "new"}
            disabled={busy}
            onClick={() => chooseTarget("new")}
          >
            {t("capture.target.new")}
          </button>
        </div>

        {loading && (
          <div className="window-capture__loading">
            <Loader2 size={13} className="window-capture__spin" />
            <span>{t("capture.loading")}</span>
          </div>
        )}

        {listFailure && (
          <div className="computer-dialog__error" role="alert">
            <ShieldAlert size={13} />
            <span>{listFailure.message}</span>
            {listFailure.cause && <span className="window-capture__cause">{listFailure.cause}</span>}
          </div>
        )}

        {list?.ok && list.warning && (
          <div className="window-capture__warn" role="status">
            {list.warning}
          </div>
        )}

        {!loading && list?.ok && windows.length === 0 && (
          <div className="computer-dialog__empty">{t("capture.noWindows")}</div>
        )}

        {windows.length > 0 && (
          <div className="computer-dialog__list">
            {windows.map((window) => {
              const key = windowCaptureKey(window);
              const active = key === selectedKey;
              const shared =
                window.shared === "read"
                  ? t("computer.shared.read")
                  : window.shared === "control"
                    ? t("computer.shared.control")
                    : "";
              return (
                <button
                  key={key}
                  type="button"
                  className={`window-capture-row${active ? " is-selected" : ""}`}
                  aria-pressed={active}
                  disabled={window.minimized}
                  onClick={() => pick(window)}
                >
                  <span className="window-capture-row__body">
                    <span className="window-capture-row__title">
                      {window.title || t("computer.untitledWindow")}
                    </span>
                    <span className="window-capture-row__sub">
                      {windowProcessName(window)}
                      {window.minimized ? ` - ${t("capture.minimizedHint")}` : ""}
                      {shared ? ` - ${shared}` : ""}
                    </span>
                  </span>
                  {active && <Check size={13} />}
                </button>
              );
            })}
          </div>
        )}

        {failure && (
          <div className="computer-dialog__error" role="alert">
            <ShieldAlert size={13} />
            <span>{failure.message}</span>
            {failure.cause && <span className="window-capture__cause">{failure.cause}</span>}
          </div>
        )}

        {lastPayload && (
          <div className="window-capture__notice" role="status">
            {lastPayload.target === "new"
              ? t("capture.successNew", { name: lastPayload.attachments[0]?.name ?? "" })
              : t("capture.successCurrent", { name: lastPayload.attachments[0]?.name ?? "" })}
          </div>
        )}

        {lastPayload?.warnings.map((warning) => (
          <div key={warning} className="window-capture__warn" role="status">
            {warningText(warning)}
          </div>
        ))}

        <div className="window-capture__shortcut">
          <Keyboard size={12} />
          <span>{t("capture.shortcut")}</span>
          <kbd>{recording ? t("capture.shortcut.recording") : describeShortcut(shortcut)}</kbd>
          <button
            type="button"
            className="computer-dialog__text-btn"
            disabled={busy}
            onClick={() => {
              setShortcutIssue(null);
              setRecording((value) => !value);
            }}
          >
            {t("capture.shortcut.change")}
          </button>
          {!shortcutValidation.ok && (
            <button type="button" className="computer-dialog__text-btn" onClick={resetShortcut}>
              {t("capture.shortcut.reset")}
            </button>
          )}
        </div>
        {activeShortcutIssue && (
          <div className="window-capture__shortcut-error" role="alert">
            {shortcutReason(activeShortcutIssue)}
          </div>
        )}

        <div className="computer-dialog__foot">
          <span className="computer-dialog__shortcut">{t("capture.privacy")}</span>
          <div className="computer-dialog__foot-actions">
            <button
              type="button"
              className="computer-dialog__text-btn"
              disabled={loading || busy}
              onClick={() => void refresh()}
            >
              <RefreshCw size={12} />
              {t("capture.refresh")}
            </button>
            <button type="button" className="project-dialog__cancel" onClick={onClose}>
              {t("common.cancel")}
            </button>
            <button
              type="button"
              className="project-dialog__submit"
              disabled={!selected || selected.minimized || busy || loading}
              onClick={() => void capture()}
            >
              {busy ? <Loader2 size={13} className="window-capture__spin" /> : <Camera size={13} />}
              {busy
                ? t("capture.capturing")
                : target === "new"
                  ? t("capture.captureNew")
                  : t("capture.capture")}
            </button>
          </div>
        </div>
      </div>
    </div>,
    document.body,
  );
}
