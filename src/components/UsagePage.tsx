import { useEffect } from "react";
import { useT } from "../lib/i18n";
import type { ScheduleAfterResetPayload } from "../lib/usageSchedule";
import { UsageV2Body } from "./usage/UsageV2Body";

/**
 * Token usage page - the pseudo tab / dock page mount. The mounting chrome owns
 * the title and close affordance (tab x / dock back); this file only lays out
 * the body. Esc closes, unless a nested control already handled it.
 */
export function UsagePage({
  onClose,
  onOpenSession,
  onScheduleAfterReset,
}: {
  onClose: () => void;
  onOpenSession?: (id: string) => void;
  /** Host hook: enqueue through the existing scheduled-send path. */
  onScheduleAfterReset?: (payload: ScheduleAfterResetPayload) => void;
}) {
  const t = useT();
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="usage-page" role="region" aria-label={t("usageLedger.analysisTitle")}>
      <div className="usage-page__scroll usage-page__scroll--flush">
        <UsageV2Body onOpenSession={onOpenSession} onScheduleAfterReset={onScheduleAfterReset} />
      </div>
    </div>
  );
}
