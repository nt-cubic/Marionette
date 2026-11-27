import { useT } from "../lib/i18n";
import { formatByteSize } from "../lib/pastedTextFold";
import type {
  SessionReferenceChipState,
  SessionReferenceTarget,
} from "../lib/sessionReferenceChip";

type Props = {
  chips: SessionReferenceChipState[];
  onRemove: (target: SessionReferenceTarget) => void;
};

function chipLabel(chip: SessionReferenceChipState): string {
  return chip.snapshot?.metadata.label || chip.target.label || chip.target.sessionId;
}

/**
 * Session-reference pills live in the existing attachment row. Styling is the
 * shared `.composer__attachment-*` language; no new design primitives.
 */
export function SessionReferenceChips({ chips, onRemove }: Props) {
  const t = useT();
  if (chips.length === 0) return null;

  return (
    <>
      {chips.map((chip) => {
        const label = chipLabel(chip);
        let badge: string;
        let title: string;

        if (chip.status === "loading") {
          badge = t("composer.sessionReferenceLoading");
          title = `${label} \u00b7 ${badge}`;
        } else if (chip.status === "error") {
          if (chip.error === "desktop_required") {
            badge = t("composer.sessionReferenceErrorDesktop");
          } else if (chip.error === "not_found") {
            badge = t("composer.sessionReferenceErrorNotFound");
          } else {
            badge = chip.errorDetail
              ? t("composer.sessionReferenceReadFailedDetail", {
                  reason: chip.errorDetail,
                })
              : t("composer.sessionReferenceErrorReadFailed");
          }
          title = `${label} \u00b7 ${badge}`;
        } else {
          const counts = chip.snapshot?.truncation;
          const turns = counts?.turnsKept ?? 0;
          const tools = counts?.toolOutcomesKept ?? 0;
          const size = formatByteSize(counts?.bytes ?? 0);
          badge = t("composer.sessionReferenceCounts", {
            turns,
            tools,
            size,
          });
          if (counts?.truncated) {
            badge = t("composer.sessionReferenceTruncated", { counts: badge });
          }
          const source = [chip.target.projectId, chip.target.agentId]
            .filter(Boolean)
            .join(" \u00b7 ");
          title = [label, source, badge].filter(Boolean).join("\n");
        }

        return (
          <span
            key={chip.target.sessionId}
            className="composer__attachment-pill composer__attachment-pill--session"
            title={title}
          >
            <span className="composer__attachment-main">
              <span className="composer__attachment-name">
                {t("composer.sessionReferenceChip", { label })}
              </span>
              <span className="composer__attachment-badge">{badge}</span>
            </span>
            <button
              type="button"
              className="composer__attachment-x"
              title={t("composer.sessionReferenceRemove", { label })}
              aria-label={t("composer.sessionReferenceRemove", { label })}
              onClick={() => onRemove(chip.target)}
            >
              {"\u2715"}
            </button>
          </span>
        );
      })}
    </>
  );
}
