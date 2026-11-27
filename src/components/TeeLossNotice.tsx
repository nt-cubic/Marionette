/**
 * P0-3: worker tee-loss notice.
 *
 * This is a session chrome strip, **not** a transcript card. It is rendered
 * from live tee health state and therefore cannot be duplicated by reopening
 * a session or by a retry (the canonical transcript is untouched). Copy is
 * honest: queue drops never reached disk and are unrecoverable; only the
 * records that did reach the ledger can be replayed.
 */
import { useT } from "../lib/i18n";
import {
  teeNoticeCopy,
  teeNoticeState,
  type ChiefTeeHealth,
} from "../lib/chiefTee";

export type TeeRetryStatus = {
  busy: boolean;
  result: "applied" | "none" | "failed" | null;
  applied?: number;
};

type TeeLossNoticeProps = {
  health: ChiefTeeHealth | null | undefined;
  status?: TeeRetryStatus;
  onRetry: () => void;
  onDismiss: () => void;
};

export function TeeLossNotice({
  health,
  status,
  onRetry,
  onDismiss,
}: TeeLossNoticeProps) {
  const t = useT();
  const state = teeNoticeState(health);
  if (!state.visible) return null;
  const copy = teeNoticeCopy(state);
  const tr = t as (
    key: string,
    params?: Record<string, string | number>,
  ) => string;
  const busy = status?.busy === true;
  return (
    <div className="tee-loss-notice" role="status">
      <div className="tee-loss-notice__copy">
        <strong>{tr(copy.titleKey, copy.titleParams)}</strong>
        <span>{tr(copy.bodyKey, copy.bodyParams)}</span>
        <span>{tr(copy.remainingKey, copy.remainingParams)}</span>
        {copy.badLinesKey ? (
          <span>{tr(copy.badLinesKey, copy.badLinesParams)}</span>
        ) : null}
        {busy ? (
          <span className="tee-loss-notice__result">
            {tr("session.teeLossRetrying")}
          </span>
        ) : status?.result === "applied" ? (
          <span className="tee-loss-notice__result">
            {tr("session.teeLossRetryApplied", {
              count: status.applied ?? 0,
            })}
          </span>
        ) : status?.result === "none" ? (
          <span className="tee-loss-notice__result">
            {tr("session.teeLossRetryNone")}
          </span>
        ) : status?.result === "failed" ? (
          <span className="tee-loss-notice__result is-failed">
            {tr("session.teeLossRetryFailed")}
          </span>
        ) : null}
      </div>
      <div className="tee-loss-notice__actions">
        <button
          type="button"
          className="tee-loss-notice__retry"
          disabled={busy || !state.canRetry}
          onClick={onRetry}
        >
          {tr(copy.retryKey)}
        </button>
        <button
          type="button"
          className="tee-loss-notice__dismiss"
          onClick={onDismiss}
        >
          {tr(copy.dismissKey)}
        </button>
      </div>
    </div>
  );
}
