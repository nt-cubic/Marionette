import { useEffect } from "react";
import { X } from "lucide-react";
import { useT } from "../lib/i18n";
import type { ScheduleAfterResetPayload } from "../lib/usageSchedule";
import { UsageV2Body } from "./usage/UsageV2Body";

/**
 * Token usage analysis in a dialog - the fallback surface for the edge dock's
 * narrow bar and detached windows that have no tab row. The page body is shared
 * with the pseudo tab / dock page, so the T3-style hierarchy is identical.
 */
export function UsageAnalysisDialog({
  onClose,
  onScheduleAfterReset,
}: {
  onClose: () => void;
  onScheduleAfterReset?: (payload: ScheduleAfterResetPayload) => void;
}) {
  const t = useT();
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="project-dialog-backdrop" onClick={onClose}>
      <div
        className="project-dialog usage-analysis-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("usageLedger.analysisTitle")}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="project-dialog__header">
          <span>{t("usageLedger.analysisTitle")}</span>
          <button
            type="button"
            className="pill-action pill-action--icon"
            aria-label={t("common.close")}
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </div>
        <div className="project-dialog__body">
          <UsageV2Body onScheduleAfterReset={onScheduleAfterReset} />
        </div>
      </div>
    </div>
  );
}
