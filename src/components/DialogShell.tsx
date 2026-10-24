import { useEffect, useRef, type ReactNode } from "react";

type DialogShellProps = {
  children: ReactNode;
  /** Called for Esc and backdrop clicks. Callers may guard this for busy states. */
  onClose: () => void;
  /** Classes for the fixed backdrop wrapper (previously each dialog's outer div). */
  className?: string;
  /** Classes for the dialog panel (previously each dialog's inner div). */
  dialogClassName?: string;
  /** Accessible name when no visible title id is available. */
  label?: string;
  /** id of the element that labels the dialog. Takes precedence over `label`. */
  labelledBy?: string;
  /** id of the element that describes the dialog. */
  describedBy?: string;
  /** ConfirmDialog uses alertdialog; everything else uses dialog. */
  role?: "dialog" | "alertdialog";
};

/**
 * Shared modal shell: backdrop, Esc, aria wiring and initial focus.
 * Keeps each caller's existing class names so no styling changes are needed.
 */
export function DialogShell({
  children,
  onClose,
  className,
  dialogClassName,
  label,
  labelledBy,
  describedBy,
  role = "dialog",
}: DialogShellProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);

  useEffect(() => {
    onCloseRef.current = onClose;
  });

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    const dialog = dialogRef.current;
    // Respect focus that children already claimed (autoFocus / ConfirmDialog cancel).
    if (dialog && !dialog.contains(document.activeElement)) {
      dialog.focus();
    }
  }, []);

  return (
    <div
      className={className}
      onClick={(event) => {
        if (event.target === event.currentTarget) onCloseRef.current();
      }}
    >
      <div
        ref={dialogRef}
        className={dialogClassName}
        role={role}
        aria-modal="true"
        aria-label={labelledBy ? undefined : label}
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        tabIndex={-1}
        onClick={(event) => event.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}
