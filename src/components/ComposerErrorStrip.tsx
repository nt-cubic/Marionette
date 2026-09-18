import type { ClassifiedError } from "../lib/errors";

type ComposerErrorStripProps = {
  error: ClassifiedError;
  canSignIn: boolean;
  onRetry: () => void;
  onSignIn?: () => void;
  onNewSession: () => void;
  onDismiss: () => void;
};

export function ComposerErrorStrip({
  error,
  canSignIn,
  onRetry,
  onSignIn,
  onNewSession,
  onDismiss,
}: ComposerErrorStripProps) {
  return (
    <div className="composer-error-strip" role="alert">
      <div className="composer-error-strip__copy">
        <strong>{error.title}</strong>
        <span>{error.actionHint || error.message}</span>
      </div>
      <div className="composer-error-strip__actions">
        <button type="button" onClick={onRetry}>
          重试
        </button>
        {canSignIn && onSignIn && error.kind === "auth" ? (
          <button type="button" onClick={onSignIn}>
            Sign in
          </button>
        ) : null}
        <button type="button" onClick={onNewSession}>
          新会话
        </button>
        <button type="button" className="composer-error-strip__dismiss" onClick={onDismiss}>
          关闭
        </button>
      </div>
    </div>
  );
}
