import { useEffect, useRef } from "react";
import { TriangleAlert } from "lucide-react";

type ConfirmDialogProps = {
  /** Dialog title, e.g. "删除对话" */
  title: string;
  /** Highlighted target name shown under the title */
  itemName?: string;
  /** Explanation text, e.g. irreversibility notice */
  description?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Danger (red) vs normal (accent) confirm button. Defaults to danger. */
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
};

/**
 * In-app confirm window replacing `window.confirm`.
 * Native confirm blocks the WebView2 thread and looks foreign —
 * this reuses the existing `project-dialog` styling instead.
 */
export function ConfirmDialog({
  title,
  itemName,
  description,
  confirmLabel = "删除",
  cancelLabel = "取消",
  danger = true,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  // Focus "取消" by default so Enter doesn't accidentally delete.
  useEffect(() => {
    cancelRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCancel();
      } else if (event.key === "Enter" && event.target instanceof HTMLElement) {
        // Let buttons handle Enter natively; plain text focus confirms.
        const tag = event.target.tagName;
        if (tag !== "BUTTON" && tag !== "INPUT" && tag !== "TEXTAREA" && tag !== "SELECT") {
          event.preventDefault();
          onConfirm();
        }
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onCancel, onConfirm]);

  return (
    <div
      className="project-dialog-backdrop confirm-dialog-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
    >
      <div
        className="project-dialog confirm-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="project-dialog__header confirm-dialog__header">
          <span className="confirm-dialog__icon" aria-hidden>
            <TriangleAlert size={16} />
          </span>
          <div>
            <strong>{title}</strong>
            {itemName && (
              <span className="confirm-dialog__item" title={itemName}>
                “{itemName}”
              </span>
            )}
          </div>
        </div>
        {description && <p className="confirm-dialog__desc">{description}</p>}
        <div className="project-dialog__actions">
          <button
            ref={cancelRef}
            type="button"
            className="project-dialog__cancel"
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            className={
              danger
                ? "project-dialog__submit project-dialog__submit--danger"
                : "project-dialog__submit"
            }
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
