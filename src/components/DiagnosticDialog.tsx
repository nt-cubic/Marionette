import { useState } from "react";
import { X } from "lucide-react";
import { revealInFileManager, saveDiagnosticReport } from "../lib/api";
import { clearCapturedUiError } from "../lib/diagnosticCapture";
import type { DiagnosticUiState } from "../lib/diagnostics";
import { useT } from "../lib/i18n";
import { showToast } from "../lib/toast";
import { DialogShell } from "./DialogShell";

type Props = {
  uiState: DiagnosticUiState;
  onClose: () => void;
};

export function DiagnosticDialog({ uiState, onClose }: Props) {
  const t = useT();
  const [summary, setSummary] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const handleSave = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const result = await saveDiagnosticReport({
        schemaVersion: uiState.schemaVersion,
        summary,
        uiState,
      });
      if (!result) {
        setBusy(false);
        return;
      }
      clearCapturedUiError();
      showToast({
        text: t("diagnostics.saved"),
        detail: t("diagnostics.savedDetail", {
          files: result.logFiles,
          redactions: result.redactions,
        }),
        tone: "ok",
        action: {
          label: t("diagnostics.reveal"),
          run: () => void revealInFileManager(result.path).catch(() => undefined),
        },
      });
      onClose();
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
      setBusy(false);
    }
  };

  return (
    <DialogShell
      className="project-dialog-backdrop"
      dialogClassName="project-dialog diagnostic-dialog"
      labelledBy="diagnostic-dialog-title"
      describedBy="diagnostic-dialog-lead"
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <div className="project-dialog__header">
        <div>
          <strong id="diagnostic-dialog-title">{t("diagnostics.title")}</strong>
          <span>{t("diagnostics.subtitle")}</span>
        </div>
        <button
          className="project-dialog__close"
          type="button"
          title={t("common.close")}
          aria-label={t("common.close")}
          disabled={busy}
          onClick={onClose}
        >
          <X size={14} />
        </button>
      </div>

      <div className="diagnostic-dialog__body">
        <p className="diagnostic-dialog__lead" id="diagnostic-dialog-lead">
          {t("diagnostics.lead")}
        </p>

        <label className="diagnostic-dialog__field">
          <span>{t("diagnostics.summary")}</span>
          <textarea
            autoFocus
            value={summary}
            onChange={(event) => setSummary(event.target.value)}
            placeholder={t("diagnostics.summaryPlaceholder")}
            disabled={busy}
          />
        </label>

        <div className="diagnostic-dialog__scope">
          <strong>{t("diagnostics.included")}</strong>
          <ul>
            <li>{t("diagnostics.includedLogs")}</li>
            <li>{t("diagnostics.includedRuntime")}</li>
            <li>{t("diagnostics.includedErrors")}</li>
          </ul>
          <p>{t("diagnostics.privacy")}</p>
        </div>

        {error && (
          <p className="project-dialog__error" role="alert">
            {t("diagnostics.failed", { error })}
          </p>
        )}
      </div>

      <div className="project-dialog__actions">
        <button
          className="project-dialog__cancel"
          type="button"
          disabled={busy}
          onClick={onClose}
        >
          {t("common.cancel")}
        </button>
        <button
          className="project-dialog__submit"
          type="button"
          disabled={busy}
          onClick={() => void handleSave()}
        >
          {busy ? t("diagnostics.saving") : t("diagnostics.save")}
        </button>
      </div>
    </DialogShell>
  );
}
